<#
    N-voke agent migration: installer. Runs from the "N-voke Agent Migration"
    scheduled task as SYSTEM (64-bit PowerShell), every TaskRepeatMinutes until
    the Windows Agent is healthy on the destination, then removes its own task.

    Reads    C:\ProgramData\N-voke\Migration\job.json      (written by the launcher)
    Writes   C:\ProgramData\N-voke\Migration\status.json   (never contains the token)
             C:\ProgramData\N-voke\Migration\logs\install-<timestamp>.log (last 10 kept)

    Exit codes: 0 healthy or waiting, 1001 download failed, 1003 install failed,
    1005 unexpected error, 1007 job.json missing or invalid.
#>
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Root = [Environment]::GetFolderPath('CommonApplicationData') + '\N-voke\Migration'
$JobPath = Join-Path $Root 'job.json'
$StatusPath = Join-Path $Root 'status.json'
$LogDir = Join-Path $Root 'logs'
$SetupPath = Join-Path $Root 'WindowsAgentSetup.exe'
$TaskName = 'N-voke Agent Migration'
$ReviewTaskName = 'N-voke Migration Cleanup'
$AgentDir = [Environment]::GetFolderPath('ProgramFilesX86') + '\N-able Technologies\Windows Agent'
$ServerConfigPath = Join-Path $AgentDir 'config\ServerConfig.xml'
$HealthGraceMinutes = 60

New-Item -ItemType Directory -Path $LogDir -Force | Out-Null
$LogPath = Join-Path $LogDir ('install-{0}.log' -f (Get-Date -Format 'yyyyMMdd-HHmmss'))
Get-ChildItem $LogDir -Filter 'install-*.log' | Sort-Object Name -Descending | Select-Object -Skip 9 | Remove-Item -Force -ErrorAction SilentlyContinue

function Write-Log([string]$Message) {
    Add-Content -Path $LogPath -Value ('{0} {1}' -f (Get-Date).ToString('o'), $Message) -Encoding UTF8
}

function Save-Status($Status) {
    $Status.updatedAt = (Get-Date).ToUniversalTime().ToString('o')
    $tmp = "$StatusPath.tmp"
    $Status | ConvertTo-Json -Depth 5 | Set-Content -Path $tmp -Encoding UTF8
    Move-Item -Path $tmp -Destination $StatusPath -Force
}

function Add-History($Status, [string]$Event, [string]$Detail) {
    $entry = [ordered]@{ at = (Get-Date).ToUniversalTime().ToString('o'); event = $Event; detail = $Detail }
    $Status.history = @(@($Status.history) + $entry | Select-Object -Last 50)
    Write-Log "$Event $Detail"
}

function Get-AgentState {
    $service = Get-Service -Name 'Windows Agent Service' -ErrorAction SilentlyContinue
    $server = $null
    if (Test-Path $ServerConfigPath) {
        try { $server = ([xml](Get-Content $ServerConfigPath -Raw)).SelectSingleNode('//ServerIP').InnerText } catch { $server = $null }
    }
    # Registry, not Win32_Product: that query is slow and can trigger MSI self-repair.
    $version = $null
    foreach ($view in 'Registry64', 'Registry32') {
        $uninstall = [Microsoft.Win32.RegistryKey]::OpenBaseKey('LocalMachine', $view).OpenSubKey('SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall')
        if (-not $uninstall) { continue }
        foreach ($name in $uninstall.GetSubKeyNames()) {
            $k = $uninstall.OpenSubKey($name)
            if ($k -and $k.GetValue('DisplayName') -eq 'Windows Agent') { $version = $k.GetValue('DisplayVersion'); break }
        }
        if ($version) { break }
    }
    $ufa = Get-Service -Name 'MspAgent' -ErrorAction SilentlyContinue
    [pscustomobject]@{
        Installed      = [bool]$version
        Version        = $version
        ServiceRunning = [bool]($service -and $service.Status -eq 'Running')
        Server         = $server
        UfaRunning     = [bool]($ufa -and $ufa.Status -eq 'Running')
    }
}

function Test-MsiBusy {
    $mutex = $null
    if ([System.Threading.Mutex]::TryOpenExisting('Global\_MSIExecute', [ref]$mutex)) { $mutex.Dispose(); return $true }
    return $false
}

function Test-RebootPending {
    foreach ($k in 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending',
                   'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired') {
        if (Test-Path $k) { return $true }
    }
    return $false
}

# Products removed by a full cleanup, in order. Matched by exact display name, never by wildcard.
$CleanupProducts = 'Windows Agent', 'MSP Core Agent', 'Patch Management Service Controller', 'Request Handler Agent', 'File Cache Service Agent'
$ProgramFilesX86 = [Environment]::GetFolderPath('ProgramFilesX86')
$CommonAppData = [Environment]::GetFolderPath('CommonApplicationData')
$CleanupFolders = @(
    "$ProgramFilesX86\N-able Technologies", "$ProgramFilesX86\Msp Agent", "$ProgramFilesX86\MspPlatform", "$ProgramFilesX86\BeAnywhere Support Express",
    "$CommonAppData\N-able Technologies", "$CommonAppData\MspAgent", "$CommonAppData\MspPlatform",
    "$CommonAppData\GetSupportService_N-Central", "$CommonAppData\GetSupportService_Common_N-Central",
    "$env:windir\System32\config\systemprofile\AppData\Local\MSPScriptExecution"
)
$CleanupRegistry = 'HKLM:\SOFTWARE\N-able Technologies', 'HKLM:\SOFTWARE\WOW6432Node\N-able Technologies'

function Get-UninstallEntry([string]$DisplayName) {
    foreach ($view in 'Registry64', 'Registry32') {
        $uninstall = [Microsoft.Win32.RegistryKey]::OpenBaseKey('LocalMachine', $view).OpenSubKey('SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall')
        if (-not $uninstall) { continue }
        foreach ($name in $uninstall.GetSubKeyNames()) {
            $k = $uninstall.OpenSubKey($name)
            if ($k -and $k.GetValue('DisplayName') -eq $DisplayName) {
                return [pscustomobject]@{ Key = $name; Msi = [bool]$k.GetValue('WindowsInstaller') -or ([string]$k.GetValue('UninstallString')) -match '^MsiExec'; Uninstall = [string]$k.GetValue('UninstallString') }
            }
        }
    }
    return $null
}

function Invoke-WithTimeout([string]$File, [string]$Arguments, [int]$Minutes = 20) {
    $p = Start-Process -FilePath $File -ArgumentList $Arguments -PassThru -WindowStyle Hidden
    if (-not $p.WaitForExit($Minutes * 60 * 1000)) { try { $p.Kill() } catch { }; return 'timeout' }
    return $p.ExitCode
}

# Removes every N-central component so the new agent installs clean and the destination
# redeploys its own UFA (an in-place upgrade keeps the old UFA enrolled in the source tenant).
function Invoke-FullCleanup($Status) {
    $log = @()
    foreach ($product in $CleanupProducts) {
        $entry = Get-UninstallEntry $product
        if (-not $entry) { continue }
        if ($entry.Msi) {
            $guid = if ($entry.Key -match '^\{[0-9A-Fa-f-]{36}\}$') { $entry.Key } else { [regex]::Match($entry.Uninstall, '\{[0-9A-Fa-f-]{36}\}').Value }
            $msiLog = Join-Path $LogDir ('uninstall-{0}.log' -f ($product -replace '\W', ''))
            $code = if (-not $guid) { 'product code not found' } else { Invoke-WithTimeout "$env:windir\System32\msiexec.exe" "/x $guid /qn REBOOT=ReallySuppress /L*V `"$msiLog`"" }
        } else {
            $exe = [regex]::Match($entry.Uninstall, '^"?([^"]+\.exe)').Groups[1].Value
            $code = if ($exe -and (Test-Path $exe)) { Invoke-WithTimeout $exe '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART' } else { 'uninstaller missing' }
        }
        $log += "uninstall $product -> $code"
        # Inno Setup uninstallers hand off to a temp copy and exit at once: wait until the entry is gone.
        for ($wait = 0; $wait -lt 60 -and (Get-UninstallEntry $product); $wait++) { Start-Sleep -Seconds 5 }
        if (Get-UninstallEntry $product) { $code = "$code (entry still present after 5 min)" }
        Add-History $Status 'cleanup-uninstall' "$product -> $code"
    }
    # Take Control has no uninstall entry; its services, helper task and folder go directly.
    Unregister-ScheduledTask -TaskName 'Take Control Agent N-Central Helper' -Confirm:$false -ErrorAction SilentlyContinue
    $leftover = Get-CimInstance Win32_Service | Where-Object { $p = [string]$_.PathName; $CleanupFolders | Where-Object { $p -like "*$_*" } }
    foreach ($svc in $leftover) {
        Stop-Service -Name $svc.Name -Force -ErrorAction SilentlyContinue
        & "$env:windir\System32\sc.exe" delete $svc.Name | Out-Null
        $log += "service $($svc.Name) deleted"
        Add-History $Status 'cleanup-service' $svc.Name
    }
    Get-Process | Where-Object { $path = $_.Path; $path -and ($CleanupFolders | Where-Object { $path -like "$_*" }) } | Stop-Process -Force -ErrorAction SilentlyContinue
    foreach ($folder in $CleanupFolders) {
        if (-not (Test-Path $folder)) { continue }
        # Handles from processes stopped a moment ago take time to close; the new install overwrites anything left.
        for ($try = 1; $try -le 6 -and (Test-Path $folder); $try++) {
            Remove-Item -Path $folder -Recurse -Force -ErrorAction SilentlyContinue
            if (Test-Path $folder) { Start-Sleep -Seconds 10 }
        }
        $result = 'removed'
        if (Test-Path $folder) {
            $left = @(Get-ChildItem $folder -Recurse -File -ErrorAction SilentlyContinue | Select-Object -First 3 | ForEach-Object { $_.FullName.Substring($folder.Length) })
            $result = "partly left (files in use: $($left -join ', '))"
        }
        $log += "folder $folder $result"
        Add-History $Status 'cleanup-folder' "$folder $result"
    }
    foreach ($key in $CleanupRegistry) {
        if (Test-Path $key) {
            Remove-Item -Path $key -Recurse -Force -ErrorAction SilentlyContinue
            Add-History $Status 'cleanup-registry' "$key $(if (Test-Path $key) { 'left' } else { 'removed' })"
        }
    }
    return $log
}

function Get-Installer([string]$Server) {
    $url = "https://$Server/download/current/winnt/N-central/WindowsAgentSetup.exe"
    if (Test-Path $SetupPath) {
        if ((Get-AuthenticodeSignature $SetupPath).Status -eq 'Valid' -and (Get-Item $SetupPath).Length -gt 1MB) { return 'reused earlier download' }
        Remove-Item $SetupPath -Force
    }
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    $method = 'BITS'
    $started = Get-Date
    try {
        Import-Module BitsTransfer -ErrorAction Stop
        Start-BitsTransfer -Source $url -Destination $SetupPath -ErrorAction Stop
    } catch {
        Write-Log "BITS download failed ($($_.Exception.Message)); falling back to Invoke-WebRequest"
        $method = 'Invoke-WebRequest'
        Invoke-WebRequest -Uri $url -OutFile $SetupPath -UseBasicParsing -TimeoutSec 1800
    }
    $sig = Get-AuthenticodeSignature $SetupPath
    if ($sig.Status -ne 'Valid') { Remove-Item $SetupPath -Force; throw "Downloaded installer signature is $($sig.Status)" }
    '{0}, {1:N1} MB in {2:N0} s, signed by {3}' -f $method, ((Get-Item $SetupPath).Length / 1MB), ((Get-Date) - $started).TotalSeconds, $sig.SignerCertificate.Subject
}

# ---------------------------------------------------------------------------
Write-Log "Installer started (PID $PID, 64-bit: $([Environment]::Is64BitProcess))"
if (-not (Test-Path $JobPath)) { Write-Log 'job.json missing'; exit 1007 }
try { $job = Get-Content $JobPath -Raw | ConvertFrom-Json } catch { Write-Log "job.json invalid: $($_.Exception.Message)"; exit 1007 }

$status = [ordered]@{}
if (Test-Path $StatusPath) {
    try { (Get-Content $StatusPath -Raw | ConvertFrom-Json).PSObject.Properties | ForEach-Object { $status[$_.Name] = $_.Value } } catch { }
}
$status.schema = 1
$status.migrationId = $job.migrationId
$status.launcherVersion = $job.launcherVersion
$status.mode = $job.mode
$status.destServer = $job.destServer
$status.destCustomerId = $job.destCustomerId
$status.attempts = [int]$status.attempts + 1
if (-not $status.history) { $status.history = @() }

try {
    $agent = Get-AgentState
    $status.agent = [ordered]@{ installed = $agent.Installed; version = $agent.Version; serviceRunning = $agent.ServiceRunning; server = $agent.Server; ufaRunning = $agent.UfaRunning }
    $onDest = $agent.Installed -and $agent.ServiceRunning -and $agent.Server -eq $job.destServer
    # Same-server reinstall: the agent already points at the destination, so only an install we ran counts.
    $weInstalled = [bool]$status.installedAt -and $status.lastInstallExitCode -eq 0

    if ($onDest -and ($job.mode -ne 'reinstall' -or $weInstalled)) {
        # The UFA installed with a new agent starts before that agent has registered and doesn't provision
        # until it's restarted; its provisioned identity is what ties it to the destination's Ecoverse tenant.
        $ufaService = Get-Service -Name 'MspAgent' -ErrorAction SilentlyContinue
        $provisionLog = [Environment]::GetFolderPath('ProgramFilesX86') + '\Msp Agent\logs\provision.log'
        $since = if ($status.installedAt) { [datetime]::Parse($status.installedAt).ToUniversalTime() } else { [datetime]::MinValue }
        $testProvisioned = { (Test-Path $provisionLog) -and (Get-Item $provisionLog).LastWriteTimeUtc -gt $since -and [bool](Select-String -Path $provisionLog -Pattern 'fetchData response received\. Status: 200' -Quiet) }
        $ufaProvisioned = & $testProvisioned
        $status.ufa = [ordered]@{ present = [bool]$ufaService; running = $agent.UfaRunning; provisioned = $ufaProvisioned }

        # Only wait for the UFA when the destination is linked to Ecoverse (the project says so); otherwise it can't provision there.
        if ($job.expectUfa -eq 'true' -and $ufaService -and -not $ufaProvisioned) {
            if (-not $status.ufaRestartedAt) {
                Restart-Service -Name 'MspAgent' -Force -ErrorAction SilentlyContinue
                $status.ufaRestartedAt = (Get-Date).ToUniversalTime().ToString('o')
                Add-History $status 'ufa-restarted' 'Windows Agent healthy on the destination; restarted MspAgent so it provisions there'
                Save-Status $status
                # Provisioning usually follows within seconds; give it five minutes in this run.
                for ($i = 0; $i -lt 30 -and -not $ufaProvisioned; $i++) { Start-Sleep -Seconds 10; $ufaProvisioned = & $testProvisioned }
                if (-not $ufaProvisioned) {
                    $status.stage = 'awaiting-ufa'
                    Add-History $status 'awaiting-ufa' 'not provisioned 5 min after restart; checking again next run'
                    Save-Status $status
                    exit 0
                }
                $status.ufa.provisioned = $true
            }
            if (-not $ufaProvisioned) {
                $waited = ((Get-Date).ToUniversalTime() - [datetime]::Parse($status.ufaRestartedAt).ToUniversalTime()).TotalMinutes
                if ($waited -lt $HealthGraceMinutes) {
                    $status.stage = 'awaiting-ufa'
                    Add-History $status 'awaiting-ufa' ('{0:N0} min since MspAgent restart; not provisioned yet' -f $waited)
                    Save-Status $status
                    exit 0
                }
                $status.warning = "MspAgent not provisioned $([math]::Round($waited)) min after restart"
            }
        }

        $status.stage = 'healthy'
        $status.healthyAt = (Get-Date).ToUniversalTime().ToString('o')
        Add-History $status 'healthy' "agent $($agent.Version) on $($agent.Server); UFA present $([bool]$ufaService), provisioned $ufaProvisioned"
        Save-Status $status
        # The token goes now. status.json and the logs stay for the review period, then a one-time task removes the folder.
        Remove-Item $JobPath -Force -ErrorAction SilentlyContinue
        Remove-Item $SetupPath -Force -ErrorAction SilentlyContinue
        $reviewDays = [int]$job.reviewDays
        if ($reviewDays -gt 0) {
            $reviewUntil = (Get-Date).AddDays($reviewDays)
            $command = "Remove-Item -LiteralPath '$Root' -Recurse -Force -ErrorAction SilentlyContinue; Unregister-ScheduledTask -TaskName '$ReviewTaskName' -Confirm:`$false"
            $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($command))
            $ps = Join-Path $env:windir 'System32\WindowsPowerShell\v1.0\powershell.exe'
            $action = New-ScheduledTaskAction -Execute $ps -Argument "-NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand $encoded"
            $trigger = New-ScheduledTaskTrigger -Once -At $reviewUntil
            $settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Minutes 30)
            $principal = New-ScheduledTaskPrincipal -UserId 'S-1-5-18' -LogonType ServiceAccount -RunLevel Highest
            Register-ScheduledTask -TaskName $ReviewTaskName -Action $action -Trigger $trigger -Settings $settings -Principal $principal -Description "N-voke: remove migration records after the review period" -Force | Out-Null
            $status.reviewUntil = $reviewUntil.ToUniversalTime().ToString('o')
            Add-History $status 'review-period' "status.json and logs kept until $($status.reviewUntil), then removed"
            Save-Status $status
        }
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
        if ($reviewDays -le 0) { Remove-Item -LiteralPath $Root -Recurse -Force -ErrorAction SilentlyContinue }
        exit 0
    }

    if ($status.stage -eq 'installed-awaiting-health' -and $status.installedAt) {
        $since = (Get-Date).ToUniversalTime() - [datetime]::Parse($status.installedAt).ToUniversalTime()
        if ($since.TotalMinutes -lt $HealthGraceMinutes) {
            Add-History $status 'waiting-for-agent' ('{0:N0} min since install; service running: {1}; server: {2}' -f $since.TotalMinutes, $agent.ServiceRunning, $agent.Server)
            Save-Status $status
            exit 0
        }
    }

    if (Get-Process -Name 'WindowsAgentSetup' -ErrorAction SilentlyContinue) {
        $status.stage = 'install-in-progress'
        Add-History $status 'install-in-progress' 'an earlier WindowsAgentSetup.exe is still running'
        Save-Status $status
        exit 0
    }
    if (Test-MsiBusy) {
        $status.stage = 'waiting-on-windows'
        Add-History $status 'waiting-on-windows' 'Windows Installer is busy (another installation in progress)'
        Save-Status $status
        exit 0
    }
    $status.rebootPending = Test-RebootPending

    $status.stage = 'downloading'
    Save-Status $status
    try {
        Add-History $status 'downloaded' (Get-Installer $job.destServer)
    } catch {
        $status.stage = 'download-failed'
        $status.lastError = $_.Exception.Message
        Add-History $status 'download-failed' $_.Exception.Message
        Save-Status $status
        exit 1001
    }

    # Everything above leaves the old agent alone. The cleanup and the installer below remove it.
    if ($job.cleanMode -ne 'none' -and -not $status.cleanedAt) {
        $status.stage = 'cleaning'
        Add-History $status 'cleanup-started' "mode $($job.cleanMode)"
        Save-Status $status
        Invoke-FullCleanup $status | Out-Null
        $status.cleanedAt = (Get-Date).ToUniversalTime().ToString('o')
        Save-Status $status
    }

    $status.stage = 'installing'
    Add-History $status 'install-started' "reboot pending: $($status.rebootPending)"
    Save-Status $status
    $msiLog = Join-Path $LogDir ('msi-{0}.log' -f (Get-Date -Format 'yyyyMMdd-HHmmss'))
    $arguments = '/s /v" /qn SERVERADDRESS={0} CUSTOMERID={1} CUSTOMERSPECIFIC=1 REGISTRATION_TOKEN={2} SERVERPROTOCOL=HTTPS /L*V {3}"' -f $job.destServer, $job.destCustomerId, $job.registrationToken, $msiLog
    $code = (Start-Process -FilePath $SetupPath -ArgumentList $arguments -Wait -PassThru -WindowStyle Hidden).ExitCode

    if ($code -eq 0 -or $code -eq 3010) {
        $status.stage = 'installed-awaiting-health'
        $status.installedAt = (Get-Date).ToUniversalTime().ToString('o')
        $status.lastInstallExitCode = 0
        Add-History $status 'install-finished' "exit $code"
        Save-Status $status
        exit 0
    }
    $status.lastInstallExitCode = $code
    if ($code -eq 1618) {
        $status.stage = 'waiting-on-windows'
        Add-History $status 'waiting-on-windows' 'installer exit 1618: another installation in progress'
        Save-Status $status
        exit 0
    }
    $status.stage = 'install-failed'
    $status.lastError = "WindowsAgentSetup.exe exit $code"
    Add-History $status 'install-failed' "exit $code"
    Save-Status $status
    exit 1003
} catch {
    $status.stage = 'error'
    $status.lastError = $_.Exception.Message
    Add-History $status 'error' $_.Exception.Message
    try { Save-Status $status } catch { }
    exit 1005
}
