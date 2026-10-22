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
$Parent = Split-Path $Root -Parent
$JobPath = Join-Path $Root 'job.json'
$StatusPath = Join-Path $Root 'status.json'
$LogDir = Join-Path $Root 'logs'
$SetupPath = Join-Path $Root 'WindowsAgentSetup.exe'
$TaskName = 'N-voke Agent Migration'
$ReviewTaskName = 'N-voke Migration Cleanup'
$AgentDir = [Environment]::GetFolderPath('ProgramFilesX86') + '\N-able Technologies\Windows Agent'
$ServerConfigPath = Join-Path $AgentDir 'config\ServerConfig.xml'
$HealthGraceMinutes = 60
$UfaSyncGraceMinutes = 120
$UfaSyncMaxRestarts = 2
$UfaSyncRestartGapMinutes = 15
$UfaBlockedWatchMinutes = 360
$UfaRepairFreshMinutes = 45
$UfaBlockedRestartAfterMinutes = 30
$UfaBlockedRepairAfterMinutes = 60
$UfaBlockedSettleMinutes = 20
$UfaRepairAfterMinutes = 30
$UfaRepairWaitMinutes = 90
$UfaRepairRestartAfterMinutes = 10
$UfaBrokenConfirmMinutes = 10
$UfaRepairMaxFinalRestarts = 2

New-Item -ItemType Directory -Path $LogDir -Force | Out-Null
$LogPath = Join-Path $LogDir ('install-{0}.log' -f (Get-Date -Format 'yyyyMMdd-HHmmss'))
Get-ChildItem $LogDir -Filter 'install-*.log' | Sort-Object Name -Descending | Select-Object -Skip 9 | Remove-Item -Force -ErrorAction SilentlyContinue

function Write-InstallLog([string]$Message) {
    Add-Content -Path $LogPath -Value ('{0} {1}' -f (Get-Date).ToString('o'), $Message) -Encoding UTF8
}

# Windows PowerShell 5.1 can follow a junction or link while deleting and empty what it points to: a link
# is removed by itself and never entered. A user can create such a link where this deletes.
function Remove-TreeSafely([string]$Path) {
    $item = Get-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue
    if (-not $item) { return }
    if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { [IO.Directory]::Delete($Path); return }
    foreach ($child in @(Get-ChildItem -LiteralPath $Path -Force -ErrorAction SilentlyContinue)) {
        if ($child.PSIsContainer) { Remove-TreeSafely $child.FullName } else { Remove-Item -LiteralPath $child.FullName -Force -ErrorAction SilentlyContinue }
    }
    Remove-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue
}

function Save-Status($Status) {
    $Status.updatedAt = (Get-Date).ToUniversalTime().ToString('o')
    $tmp = "$StatusPath.tmp"
    $Status | ConvertTo-Json -Depth 5 | Set-Content -Path $tmp -Encoding UTF8
    Move-Item -Path $tmp -Destination $StatusPath -Force
}

function Add-StatusHistory($Status, [string]$EventName, [string]$Detail) {
    $entry = [ordered]@{ at = (Get-Date).ToUniversalTime().ToString('o'); event = $EventName; detail = $Detail }
    $Status.history = @(@($Status.history) + $entry | Select-Object -Last 50)
    Write-InstallLog "$EventName $Detail"
}

# The registration token is on WindowsAgentSetup.exe's command line, so its verbose MSI log can record
# it. Blank it out of every log in the folder: right after each install attempt, and at the start of
# every run in case an earlier run was killed before it could. Returns how many files it changed.
function Hide-RegistrationToken([string]$Directory, [string]$Token) {
    if ([string]::IsNullOrEmpty($Token) -or -not (Test-Path -LiteralPath $Directory)) { return 0 }
    $changed = 0
    foreach ($file in Get-ChildItem -LiteralPath $Directory -Filter '*.log' -File -ErrorAction SilentlyContinue) {
        try {
            $bytes = [IO.File]::ReadAllBytes($file.FullName)
            # MSI logs are ANSI or UTF-16: Latin-1 maps bytes one to one, so non-token bytes survive untouched.
            $utf16 = $bytes.Length -ge 2 -and $bytes[0] -eq 0xFF -and $bytes[1] -eq 0xFE
            $encoding = if ($utf16) { [Text.Encoding]::Unicode } else { [Text.Encoding]::GetEncoding(28591) }
            $text = $encoding.GetString($bytes)
            if ($text.IndexOf($Token, [StringComparison]::Ordinal) -lt 0) { continue }
            $tmp = $file.FullName + '.tmp'
            [IO.File]::WriteAllBytes($tmp, $encoding.GetBytes($text.Replace($Token, '[token removed]')))
            Move-Item -LiteralPath $tmp -Destination $file.FullName -Force
            $changed++
        } catch {
            Write-InstallLog "could not remove the registration token from $($file.Name): $($_.Exception.Message)"
        }
    }
    return $changed
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

# Reads the Ecoverse agent's core log for the one fault seen in the lab: its configuration fetch timed out while
# fetching a login token, so the inventory components were never delivered and the asset stays CONNECTED with no
# data. The agent retries that fetch only hourly; restarting MspAgent retries it at once. Only lines after $Since count.
#   synced   the inventory component (generic-asset-interrogator) was downloaded
#   stalled  a configuration fetch failed on a timeout, the cloud answers again, and nothing was delivered since: restart
#   blocked  a fetch failed and the cloud still doesn't answer. It often clears by itself (the lab saw it), so it is
#            watched first. Only if it lasts, one MspAgent restart is tried, and after that gets time to prove itself
#            the one-time repair is done (Test-UfaBlockedRestartDue, Test-UfaRepairDue). If it turns into a state
#            below, that state's rule applies
#   rejected the cloud answers the agent's login with "400 The SSL certificate error" and never accepts it: a restart
#            or a reboot doesn't help (seen in the lab). What did: removing the UFA and letting the Windows Agent redeploy it,
#            which issues a new certificate (Invoke-UfaRepair, once, after a sustained refusal)
#   denied   the login works but the component CDN answers the agent's downloads with 403 (five or more, none succeeded
#            since): nothing can be delivered. A restart changed nothing in the lab; the repair fixed it
#   waiting  no fault and no inventory component yet (normal for a while)
#   nodevice the login works and the components were downloaded, but the cloud's reply for this agent has no device record
#            (no "GOAT NRI"), so every attempt to send data fails: the component log shows "failed to get goat NRI" at
#            least twice in the last 30 minutes. This outranks synced: the inventory component downloads fine and
#            collects, it just can't send. Normal at first: on healthy lab devices the record arrived 86 to 94 minutes
#            after provisioning (the agent asks for it at provisioning and at its hourly update check). Still missing
#            after 4 hours on devices moved into one N-central's Ecoverse tenant, and the one-time repair changed
#            nothing there. A restart would make the agent ask at once (it only helps if the record exists), but none
#            is tried: the watcher only reports it
#   unknown  no log to read
function Get-UfaSyncState([string]$LogPath, [datetime]$Since, [string]$ComponentLogPath = '', [datetime]$NowUtc = [datetime]::UtcNow) {
    if (-not (Test-Path -LiteralPath $LogPath)) { return [pscustomobject]@{ State = 'unknown'; FailedAt = $null; RejectedSince = $null; FaultSince = $null; FaultLastAt = $null } }
    $stamp = [regex]'^\[(\d{4}-\d\d-\d\dT[\d:.]+[+-]\d\d:\d\d)\]'
    $failedAt = $null; $failingSince = $null; $jwtFailedAt = $null; $authOkAt = $null; $syncedAt = $null; $rejectedAt = $null; $rejectedSince = $null; $deniedCount = 0; $deniedSince = $null; $deniedAt = $null
    foreach ($line in @(Get-Content -LiteralPath $LogPath -Tail 20000 -ErrorAction SilentlyContinue)) {
        $m = $stamp.Match($line)
        if (-not $m.Success) { continue }
        $at = [datetimeoffset]::Parse($m.Groups[1].Value, [Globalization.CultureInfo]::InvariantCulture).UtcDateTime
        if ($at -lt $Since) { continue }
        if ($line -match 'auth::fetchJWTWithRetries error occurred when fetching jwt:') { $jwtFailedAt = $at }
        if ($line -match 'ERROR \(\d+\): configservice::FetchAllConfigurations failed to fetch configuration data:') { $failedAt = $at; if (-not $failingSince) { $failingSince = $at } }
        elseif ($line -match 'auth::fetchJWTWithRetries error occurred when fetching jwt: unexpected status code: 400 .*The SSL certificate error') { $rejectedAt = $at; if (-not $rejectedSince) { $rejectedSince = $at } }
        elseif ($line -match 'auth::requestOauthToken Oauth token response received\. Status: 200') { $authOkAt = $at; $rejectedSince = $null; $failingSince = $null }
        elseif ($line -match 'ERROR \(\d+\): components::doRequestWithRetry received invalid status: ''403''') { $deniedCount++; $deniedAt = $at; if (-not $deniedSince) { $deniedSince = $at } }
        elseif ($line -match 'components::Download Successfully downloaded') {
            $deniedCount = 0; $deniedSince = $null
            if ($line -match 'generic-asset-interrogator') { $syncedAt = $at }
        }
    }
    # The components' own log: each flush of their data starts by asking for the device's identity and, without one, stops.
    $noDeviceCount = 0; $noDeviceSince = $null; $noDeviceAt = $null
    if ($ComponentLogPath -and (Test-Path -LiteralPath $ComponentLogPath)) {
        $windowStart = $NowUtc.AddMinutes(-30)
        foreach ($line in @(Get-Content -LiteralPath $ComponentLogPath -Tail 5000 -ErrorAction SilentlyContinue)) {
            if ($line -notmatch 'ERROR \(\d+\): messaging::refreshDeviceInfo failed to get goat NRI') { continue }
            $m = $stamp.Match($line)
            if (-not $m.Success) { continue }
            $at = [datetimeoffset]::Parse($m.Groups[1].Value, [Globalization.CultureInfo]::InvariantCulture).UtcDateTime
            if ($at -lt $Since -or $at -lt $windowStart) { continue }
            $noDeviceCount++; $noDeviceAt = $at; if (-not $noDeviceSince) { $noDeviceSince = $at }
        }
    }
    $state = if ($noDeviceCount -ge 2) { 'nodevice' }
             elseif ($syncedAt -and (-not $failedAt -or $syncedAt -gt $failedAt)) { 'synced' }
             elseif ($failedAt -and $authOkAt -and $authOkAt -gt $failedAt) { 'stalled' }
             elseif ($rejectedAt -and (-not $authOkAt -or $authOkAt -lt $rejectedAt)) { 'rejected' }
             elseif ($deniedCount -ge 5) { 'denied' }
             elseif ($failedAt) { 'blocked' }
             else { 'waiting' }
    $faultSince = if ($state -eq 'rejected') { $rejectedSince } elseif ($state -eq 'denied') { $deniedSince } elseif ($state -eq 'blocked') { $failingSince } elseif ($state -eq 'nodevice') { $noDeviceSince } else { $null }
    # A blocked agent logs a failed login about every 10 minutes but a failed configuration fetch only about hourly.
    $blockedLastAt = if ($jwtFailedAt -and $jwtFailedAt -gt $failedAt) { $jwtFailedAt } else { $failedAt }
    $faultLastAt = if ($state -eq 'rejected') { $rejectedAt } elseif ($state -eq 'denied') { $deniedAt } elseif ($state -eq 'blocked') { $blockedLastAt } elseif ($state -eq 'nodevice') { $noDeviceAt } else { $null }
    [pscustomobject]@{ State = $state; FailedAt = $failedAt; RejectedSince = $rejectedSince; FaultSince = $faultSince; FaultLastAt = $faultLastAt }
}

# True when the same fault (login refused, component downloads refused, or a long blocked state) has lasted
# AfterMinutes with nothing getting through, no repair was done yet, and the fault is still current: its last log
# line is at most FreshMinutes old. That check keeps a refusal seen earlier, followed by an outage, from triggering a
# repair that can't help. A blocked state qualifies only when BlockedAfterMinutes is given, and then only if the
# MspAgent restart was tried at least SettleMinutes ago and a failure was logged after it (the restart did not fix it).
# Stalled, waiting and synced never qualify.
function Test-UfaRepairDue($Sync, [datetime]$NowUtc, [bool]$AlreadyRepaired, [int]$AfterMinutes, [int]$FreshMinutes = 45, [int]$BlockedAfterMinutes = 0, [int]$Restarts = 0, [datetime]$LastRestartUtc = [datetime]::MinValue, [int]$SettleMinutes = 20) {
    if ($AlreadyRepaired -or $Sync.State -notin 'rejected', 'denied', 'blocked' -or -not $Sync.FaultSince -or -not $Sync.FaultLastAt) { return $false }
    if (($NowUtc - $Sync.FaultLastAt).TotalMinutes -gt $FreshMinutes) { return $false }
    if ($Sync.State -eq 'blocked') {
        if ($BlockedAfterMinutes -le 0 -or $Restarts -lt 1) { return $false }
        if (($NowUtc - $LastRestartUtc).TotalMinutes -lt $SettleMinutes -or $Sync.FaultLastAt -le $LastRestartUtc) { return $false }
        return (($NowUtc - $Sync.FaultSince).TotalMinutes -ge $BlockedAfterMinutes)
    }
    return (($NowUtc - $Sync.FaultSince).TotalMinutes -ge $AfterMinutes)
}

# The one MspAgent restart tried for a blocked state that lasts: it makes the agent fetch its configuration now, which
# is the only thing that can fix a blocked agent short of the repair. Only once, only while the failures are current.
function Test-UfaBlockedRestartDue($Sync, [int]$Restarts, [datetime]$NowUtc, [int]$AfterMinutes, [int]$FreshMinutes = 45) {
    if ($Sync.State -ne 'blocked' -or $Restarts -gt 0 -or -not $Sync.FaultSince -or -not $Sync.FaultLastAt) { return $false }
    if (($NowUtc - $Sync.FaultLastAt).TotalMinutes -gt $FreshMinutes) { return $false }
    return (($NowUtc - $Sync.FaultSince).TotalMinutes -ge $AfterMinutes)
}

# The repair N-able support gave for a UFA whose login is refused: uninstall the UFA core, wait, then restart the Windows
# Agent services so the agent redeploys it. The new UFA provisions a fresh certificate under the same GUID.
function Restart-WindowsAgentService {
    foreach ($name in 'Windows Agent Maintenance Service', 'Windows Agent Service') {
        Restart-Service -DisplayName $name -Force -ErrorAction SilentlyContinue
        Start-Sleep -Seconds 5
    }
}

# The sequence, in this order: msp-agent-core.exe --uninstall, wait SettleSeconds (120), restart Windows Agent
# Maintenance Service, wait 5 s, restart Windows Agent Service. The uninstaller exits before the removal is over, and an
# agent restarted too early doesn't redeploy the UFA (seen in the lab: the operator always needed one more restart a
# while later), hence the fixed wait. The caller then watches the Msp Agent folder for new data (Get-UfaFolderActivity).
# Never delete that folder or msp-agent-core.exe here: --uninstall leaves them behind, and in the lab, with them removed
# by hand the Windows Agent did not redeploy the UFA; with them restored and both services restarted, it did.
function Invoke-UfaRepair([int]$SettleSeconds = 120) {
    $result = Invoke-UfaCoreUninstall
    Start-Sleep -Seconds $SettleSeconds
    $script:UfaRepairRestartedAt = (Get-Date).ToUniversalTime()
    Restart-WindowsAgentService
    return "uninstall: $result; waited $SettleSeconds s, then restarted the Windows Agent services"
}

# What the Windows Agent has written into the Msp Agent folder since SinceUtc: the sign that it is redeploying the UFA.
# The leftover msp-agent-core.exe is old, so it doesn't count.
function Get-UfaFolderActivity([string]$Path, [datetime]$SinceUtc) {
    $changed = @(Get-ChildItem -LiteralPath $Path -Recurse -File -Force -ErrorAction SilentlyContinue | Where-Object { $_.LastWriteTimeUtc -gt $SinceUtc })
    $newest = if ($changed.Count -gt 0) { ($changed | Measure-Object -Property LastWriteTimeUtc -Maximum).Maximum } else { $null }
    [pscustomobject]@{ Files = $changed.Count; NewestAt = $newest }
}

# After a repair the UFA is only redeployed when the Windows Agent notices it is missing. If it hasn't been put back
# some minutes after the last restart, restart the agent services again (bounded). Not while the Msp Agent folder is
# still being written (its newest file is younger than AfterMinutes): the redeploy is under way.
function Test-UfaRedeployRestartDue([bool]$UfaPresent, [datetime]$LastActionUtc, [int]$Restarts, [datetime]$NowUtc, [int]$AfterMinutes, [int]$MaxRestarts, $FolderNewestUtc = $null) {
    if ($UfaPresent -or $Restarts -ge $MaxRestarts) { return $false }
    if ($FolderNewestUtc -and (($NowUtc - $FolderNewestUtc).TotalMinutes -lt $AfterMinutes)) { return $false }
    return (($NowUtc - $LastActionUtc).TotalMinutes -ge $AfterMinutes)
}

# The state SUP-2190 ended up in (lab, 2026-10-21): the Ecoverse agent (the MSP Core Agent MSI) is registered, its program
# msp-agent-core.exe is gone and there is no MspAgent service. The Windows Agent then only tries to remove it, and that removal
# runs the exe, so it fails (MSI error 1721) and nothing new is ever installed. Reported, never repaired by the installer.
function Test-UfaRegistrationBroken([bool]$Registered, [bool]$ExePresent, [bool]$ServiceExists) {
    return ($Registered -and -not $ExePresent -and -not $ServiceExists)
}

# Seen on two runs at least ConfirmMinutes apart, so a removal or install that is merely in progress isn't reported.
function Test-UfaBrokenConfirmed([bool]$Broken, $FirstSeenUtc, [datetime]$NowUtc, [int]$ConfirmMinutes) {
    if (-not $Broken -or -not $FirstSeenUtc) { return $false }
    return (($NowUtc - $FirstSeenUtc).TotalMinutes -ge $ConfirmMinutes)
}

# What Windows Installer logged for the Ecoverse agent's product: 11725 removal failed (with 11721 or 11722 carrying the MSI
# error: 1721 the program could not be run, 1722 it ran and failed), 11724 removal completed. Events are objects with Id and TimeCreated.
function Get-UfaRemovalSummary($Events) {
    $failed = @($Events | Where-Object { $_.Id -eq 11725 } | Sort-Object TimeCreated)
    $coded = @($Events | Where-Object { $_.Id -eq 11721 -or $_.Id -eq 11722 } | Sort-Object TimeCreated)
    [pscustomobject]@{
        Failures      = $failed.Count
        LastErrorCode = $(if ($coded.Count -gt 0) { [int]$coded[$coded.Count - 1].Id - 10000 } else { $null })
        LastFailureAt = $(if ($failed.Count -gt 0) { $failed[$failed.Count - 1].TimeCreated.ToUniversalTime() } else { $null })
        Completed     = @($Events | Where-Object { $_.Id -eq 11724 }).Count
    }
}

# Read-only: the Application log's MsiInstaller events for the MSP Core Agent since SinceUtc.
function Get-UfaRemovalEvent([datetime]$SinceUtc) {
    if ($SinceUtc -le [datetime]::MinValue.AddDays(2)) { $SinceUtc = (Get-Date).ToUniversalTime().AddDays(-1) }
    try {
        @(Get-WinEvent -FilterHashtable @{ LogName = 'Application'; ProviderName = 'MsiInstaller'; StartTime = $SinceUtc.ToLocalTime() } -ErrorAction Stop |
            Where-Object { ($_.Id -in 11721, 11722, 11724, 11725) -and $_.Message -match 'MSP Core Agent' })
    } catch { @() }
}

# Read-only: is the Ecoverse agent registered, is its program there, does its service exist.
function Get-UfaRegistration {
    $entry = Get-UninstallEntry 'MSP Core Agent'
    $core = [Environment]::GetFolderPath('ProgramFilesX86') + '\Msp Agent\msp-agent-core.exe'
    [pscustomobject]@{
        Registered    = [bool]$entry
        ProductCode   = $(if ($entry -and $entry.Key -match '^\{[0-9A-Fa-f-]{36}\}$') { $entry.Key } else { $null })
        Version       = $(if ($entry) { $entry.Version } else { $null })
        ExePresent    = (Test-Path -LiteralPath $core)
        ServiceExists = [bool](Get-Service -Name 'MspAgent' -ErrorAction SilentlyContinue)
    }
}

function Test-RebootPending {
    foreach ($k in 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending',
                   'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired') {
        if (Test-Path $k) { return $true }
    }
    return $false
}

# Products removed by a full cleanup, in order. Matched by exact display name, never by wildcard. Only the legacy
# N-central agent and its companions: the Ecoverse agent (UFA, "MSP Core Agent") is touchy and is removed only by its own
# msp-agent-core.exe --uninstall (Invoke-UfaCoreUninstall), never by an MSI uninstall or by deleting its folders.
$CleanupProducts = 'Windows Agent', 'Patch Management Service Controller', 'Request Handler Agent', 'File Cache Service Agent'
$ProgramFilesX86 = [Environment]::GetFolderPath('ProgramFilesX86')
$CommonAppData = [Environment]::GetFolderPath('CommonApplicationData')
# The legacy N-central agent's folders only. Never the Ecoverse agent's (Program Files (x86)\Msp Agent, ProgramData\MspAgent,
# SYSTEM's MSPScriptExecution): in the lab a UFA whose folder this cleanup deleted before its own uninstall had finished
# was never redeployed by the new Windows Agent, while one whose folder was left to the Windows Agent was.
$CleanupFolders = @(
    "$ProgramFilesX86\N-able Technologies", "$ProgramFilesX86\MspPlatform",
    "$CommonAppData\N-able Technologies", "$CommonAppData\MspPlatform"
)
# What the UFA leaves behind: reported after the cleanup, never touched.
$UfaLeftovers = @("$ProgramFilesX86\Msp Agent", "$CommonAppData\MspAgent")
$CleanupRegistry = 'HKLM:\SOFTWARE\N-able Technologies', 'HKLM:\SOFTWARE\WOW6432Node\N-able Technologies'

function Get-UninstallEntry([string]$DisplayName) {
    foreach ($view in 'Registry64', 'Registry32') {
        $uninstall = [Microsoft.Win32.RegistryKey]::OpenBaseKey('LocalMachine', $view).OpenSubKey('SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall')
        if (-not $uninstall) { continue }
        foreach ($name in $uninstall.GetSubKeyNames()) {
            $k = $uninstall.OpenSubKey($name)
            if ($k -and $k.GetValue('DisplayName') -eq $DisplayName) {
                return [pscustomobject]@{ Key = $name; Msi = [bool]$k.GetValue('WindowsInstaller') -or ([string]$k.GetValue('UninstallString')) -match '^MsiExec'; Uninstall = [string]$k.GetValue('UninstallString'); Version = [string]$k.GetValue('DisplayVersion') }
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

# Removes the Ecoverse agent (UFA) with its own uninstaller, which N-able support gives as the way to take it off a
# device so it can be provisioned again with a new identity. Leftover folders and registry keys alone kept a stale
# identity in the lab (the cloud then refused its login), so this runs first, while msp-agent-core.exe still exists.
function Invoke-UfaCoreUninstall {
    $core = [Environment]::GetFolderPath('ProgramFilesX86') + '\Msp Agent\msp-agent-core.exe'
    if (-not (Test-Path -LiteralPath $core)) { return 'msp-agent-core.exe not found' }
    $code = Invoke-WithTimeout $core '--uninstall' 5
    Start-Sleep -Seconds 5
    $service = if (Get-Service -Name 'MspAgent' -ErrorAction SilentlyContinue) { 'MspAgent service still present' } else { 'MspAgent service gone' }
    return "exit code $code; $service"
}

# Removes every N-central component so the new agent installs clean and the destination
# redeploys its own UFA (an in-place upgrade keeps the old UFA enrolled in the source tenant).
function Invoke-FullCleanup($Status) {
    $log = @()
    $ufaResult = Invoke-UfaCoreUninstall
    $log += "ufa core uninstall -> $ufaResult"
    Add-StatusHistory $Status 'cleanup-ufa-uninstall' "msp-agent-core.exe --uninstall -> $ufaResult"
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
        Add-StatusHistory $Status 'cleanup-uninstall' "$product -> $code"
    }
    # The Windows Agent's uninstall hands the Ecoverse agent's own MSI removal on, and that can still be running after the
    # uninstall returns: on SUP-2190 it failed (MSI error 1722) in the same second this cleanup stopped processes and deleted
    # folders, and every later attempt failed with 1721 because msp-agent-core.exe was gone. So wait for the entry to go,
    # for at most 5 minutes, and never remove it or touch its folders ourselves.
    $ufaWaited = 0
    while ($ufaWaited -lt 60 -and (Get-UninstallEntry 'MSP Core Agent')) { Start-Sleep -Seconds 5; $ufaWaited++ }
    if ($ufaWaited -gt 0) {
        $ufaGone = -not (Get-UninstallEntry 'MSP Core Agent')
        Add-StatusHistory $Status 'cleanup-ufa-wait' $(if ($ufaGone) { "the Ecoverse agent's own removal finished after $($ufaWaited * 5) s" } else { "the Ecoverse agent is still registered after $($ufaWaited * 5) s; left alone" })
    }
    # Whatever service is still registered from an N-able folder goes too.
    $leftover = Get-CimInstance Win32_Service | Where-Object { $p = [string]$_.PathName; $CleanupFolders | Where-Object { $p -like "*$_*" } }
    foreach ($svc in $leftover) {
        Stop-Service -Name $svc.Name -Force -ErrorAction SilentlyContinue
        & "$env:windir\System32\sc.exe" delete $svc.Name | Out-Null
        $log += "service $($svc.Name) deleted"
        Add-StatusHistory $Status 'cleanup-service' $svc.Name
    }
    Get-Process | Where-Object { $path = $_.Path; $path -and ($CleanupFolders | Where-Object { $path -like "$_*" }) } | Stop-Process -Force -ErrorAction SilentlyContinue
    foreach ($folder in $CleanupFolders) {
        if (-not (Test-Path $folder)) { continue }
        # Handles from processes stopped a moment ago take time to close; the new install overwrites anything left.
        for ($try = 1; $try -le 6 -and (Test-Path $folder); $try++) {
            Remove-TreeSafely $folder
            if (Test-Path $folder) { Start-Sleep -Seconds 10 }
        }
        $result = 'removed'
        if (Test-Path $folder) {
            $left = @(Get-ChildItem $folder -Recurse -File -ErrorAction SilentlyContinue | Select-Object -First 3 | ForEach-Object { $_.FullName.Substring($folder.Length) })
            $result = "partly left (files in use: $($left -join ', '))"
        }
        $log += "folder $folder $result"
        Add-StatusHistory $Status 'cleanup-folder' "$folder $result"
    }
    # A folder that was already gone logs nothing above, so say what the old agent's folders look like now.
    $remaining = @($CleanupFolders | Where-Object { Test-Path $_ })
    Add-StatusHistory $Status 'cleanup-verify' $(if ($remaining) { "still present: $($remaining -join ', ')" } else { 'none of the legacy agent folders are present' })
    # The Ecoverse agent is left alone, but its state after the uninstall is the evidence if it never comes back.
    $ufaLeft = @($UfaLeftovers | Where-Object { Test-Path -LiteralPath $_ })
    $ufaSvc = if (Get-Service -Name 'MspAgent' -ErrorAction SilentlyContinue) { 'MspAgent service present' } else { 'MspAgent service gone' }
    $ufaProduct = if (Get-UninstallEntry 'MSP Core Agent') { 'MSP Core Agent still registered' } else { 'MSP Core Agent not registered' }
    Add-StatusHistory $Status 'cleanup-ufa-leftover' ('Ecoverse agent folders left in place on purpose: {0}; {1}; {2}' -f $(if ($ufaLeft) { $ufaLeft -join ', ' } else { 'none present' }), $ufaSvc, $ufaProduct)
    foreach ($key in $CleanupRegistry) {
        if (Test-Path $key) {
            Remove-Item -Path $key -Recurse -Force -ErrorAction SilentlyContinue
            Add-StatusHistory $Status 'cleanup-registry' "$key $(if (Test-Path $key) { 'left' } else { 'removed' })"
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
        Write-InstallLog "BITS download failed ($($_.Exception.Message)); falling back to Invoke-WebRequest"
        $method = 'Invoke-WebRequest'
        Invoke-WebRequest -Uri $url -OutFile $SetupPath -UseBasicParsing -TimeoutSec 1800
    }
    $sig = Get-AuthenticodeSignature $SetupPath
    if ($sig.Status -ne 'Valid') { Remove-Item $SetupPath -Force; throw "Downloaded installer signature is $($sig.Status)" }
    '{0}, {1:N1} MB in {2:N0} s, signed by {3}' -f $method, ((Get-Item $SetupPath).Length / 1MB), ((Get-Date) - $started).TotalSeconds, $sig.SignerCertificate.Subject
}

# ---------------------------------------------------------------------------
Write-InstallLog "Installer started (PID $PID, 64-bit: $([Environment]::Is64BitProcess))"
if (-not (Test-Path $JobPath)) { Write-InstallLog 'job.json missing'; exit 1007 }
try { $job = Get-Content $JobPath -Raw | ConvertFrom-Json } catch { Write-InstallLog "job.json invalid: $($_.Exception.Message)"; exit 1007 }
Hide-RegistrationToken $LogDir $job.registrationToken | Out-Null

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

        # After a repair the UFA is gone until the Windows Agent redeploys it: keep waiting, don't call that healthy.
        if ($job.expectUfa -eq 'true' -and $status.ufaRepairedAt -and -not ($ufaService -and $ufaProvisioned)) {
            $sinceRepair = ((Get-Date).ToUniversalTime() - [datetime]::Parse($status.ufaRepairedAt).ToUniversalTime()).TotalMinutes
            $lastAction = if ($status.ufaRepairRestartedAt) { [datetime]::Parse($status.ufaRepairRestartedAt).ToUniversalTime() } else { [datetime]::Parse($status.ufaRepairedAt).ToUniversalTime() }
            # Watch the Msp Agent folder: new files since the last restart mean the Windows Agent is redeploying the UFA.
            $activity = Get-UfaFolderActivity ([Environment]::GetFolderPath('ProgramFilesX86') + '\Msp Agent') $lastAction
            $status.ufa.redeployFiles = $activity.Files
            $status.ufa.redeployNewestAt = if ($activity.NewestAt) { $activity.NewestAt.ToUniversalTime().ToString('o') } else { $null }
            $folderNote = if ($activity.Files -gt 0) { "{0} file(s) written to the Msp Agent folder since the last restart, newest {1:HH:mm:ss}" -f $activity.Files, $activity.NewestAt.ToLocalTime() } else { 'no new data in the Msp Agent folder since the last restart' }
            if (Test-UfaRedeployRestartDue ([bool]$ufaService) $lastAction ([int]$status.ufaRepairRestarts) (Get-Date).ToUniversalTime() $UfaRepairRestartAfterMinutes $UfaRepairMaxFinalRestarts $activity.NewestAt) {
                Restart-WindowsAgentService
                $status.ufaRepairRestarts = [int]$status.ufaRepairRestarts + 1
                $status.ufaRepairRestartedAt = (Get-Date).ToUniversalTime().ToString('o')
                Add-StatusHistory $status 'ufa-repair-restart' ('MspAgent was not redeployed {0:N0} min after the repair ({1}); restarted the Windows Agent services again (restart {2} of {3})' -f $sinceRepair, $folderNote, $status.ufaRepairRestarts, $UfaRepairMaxFinalRestarts)
            }
            if ($sinceRepair -lt $UfaRepairWaitMinutes) {
                $status.stage = 'awaiting-ufa'
                Add-StatusHistory $status 'awaiting-ufa' ('{0:N0} min since the UFA repair; MspAgent not redeployed and provisioned yet; {1}' -f $sinceRepair, $folderNote)
                Save-Status $status
                exit 0
            }
            $status.warning = "MspAgent not redeployed and provisioned $([math]::Round($sinceRepair)) min after the repair"
        }

        # The Windows Agent installs the UFA a few minutes after itself. When the destination is linked, a UFA that isn't
        # there yet is not a healthy device: wait for it, up to the same limit as the data sync. One state can't fix itself
        # (SUP-2190, see Test-UfaRegistrationBroken): the Ecoverse agent is registered but its program is gone, so the Windows
        # Agent never installs a new one. That is reported plainly, with what Windows Installer logged, and never repaired here.
        if ($job.expectUfa -eq 'true' -and -not $ufaService -and -not $status.ufaRepairedAt) {
            $nowUtc = (Get-Date).ToUniversalTime()
            $waitedAbsent = ($nowUtc - $since).TotalMinutes
            $reg = Get-UfaRegistration
            $broken = Test-UfaRegistrationBroken $reg.Registered $reg.ExePresent $reg.ServiceExists
            if ($broken -and -not $status.ufaBrokenFirstSeenAt) { $status.ufaBrokenFirstSeenAt = $nowUtc.ToString('o') }
            if (-not $broken) { $status.Remove('ufaBrokenFirstSeenAt') }
            $firstSeen = if ($status.ufaBrokenFirstSeenAt) { [datetime]::Parse($status.ufaBrokenFirstSeenAt).ToUniversalTime() } else { $null }
            $confirmed = Test-UfaBrokenConfirmed $broken $firstSeen $nowUtc $UfaBrokenConfirmMinutes
            $status.ufa.brokenRegistration = $confirmed
            $brokenNote = ''
            if ($confirmed) {
                $removal = Get-UfaRemovalSummary (Get-UfaRemovalEvent $since)
                $status.ufa.removalFailures = $removal.Failures
                $status.ufa.removalError = $removal.LastErrorCode
                $removalText = 'Windows Installer removal failures since the install: {0}{1}' -f $removal.Failures, $(if ($removal.LastErrorCode) { ", last MSI error $($removal.LastErrorCode)" } else { '' })
                $brokenNote = "; the Ecoverse agent is registered but msp-agent-core.exe is missing, so the Windows Agent cannot install a new one ($removalText)"
                if (-not $status.ufaBrokenReportedAt) {
                    $status.ufaBrokenReportedAt = $nowUtc.ToString('o')
                    Add-StatusHistory $status 'ufa-registration-broken' ('MSP Core Agent {0} is registered but msp-agent-core.exe is missing and there is no MspAgent service (seen since {1:o}). The Windows Agent can only try to remove it, which runs that program, and will not install a new one. {2}. See the troubleshooting guide for the recovery steps' -f $reg.Version, $firstSeen, $removalText)
                }
            }
            if ($waitedAbsent -lt $UfaSyncGraceMinutes) {
                $status.stage = 'awaiting-ufa'
                Add-StatusHistory $status 'awaiting-ufa' ('MspAgent is not installed yet; {0:N0} min since install{1}' -f $waitedAbsent, $brokenNote)
                Save-Status $status
                exit 0
            }
            $status.warning = "MspAgent was not installed $([math]::Round($waitedAbsent)) min after the install$(if ($confirmed) { ' (the Ecoverse agent is registered but msp-agent-core.exe is missing)' })"
        }

        # Only wait for the UFA when the destination is linked to Ecoverse (the project says so); otherwise it can't provision there.
        if ($job.expectUfa -eq 'true' -and $ufaService -and -not $ufaProvisioned) {
            if (-not $status.ufaRestartedAt) {
                Restart-Service -Name 'MspAgent' -Force -ErrorAction SilentlyContinue
                $status.ufaRestartedAt = (Get-Date).ToUniversalTime().ToString('o')
                Add-StatusHistory $status 'ufa-restarted' 'Windows Agent healthy on the destination; restarted MspAgent so it provisions there'
                Save-Status $status
                # Provisioning usually follows within seconds; give it five minutes in this run.
                for ($i = 0; $i -lt 30 -and -not $ufaProvisioned; $i++) { Start-Sleep -Seconds 10; $ufaProvisioned = & $testProvisioned }
                if (-not $ufaProvisioned) {
                    $status.stage = 'awaiting-ufa'
                    Add-StatusHistory $status 'awaiting-ufa' 'not provisioned 5 min after restart; checking again next run'
                    Save-Status $status
                    exit 0
                }
                $status.ufa.provisioned = $true
            }
            if (-not $ufaProvisioned) {
                $waited = ((Get-Date).ToUniversalTime() - [datetime]::Parse($status.ufaRestartedAt).ToUniversalTime()).TotalMinutes
                if ($waited -lt $HealthGraceMinutes) {
                    $status.stage = 'awaiting-ufa'
                    Add-StatusHistory $status 'awaiting-ufa' ('{0:N0} min since MspAgent restart; not provisioned yet' -f $waited)
                    Save-Status $status
                    exit 0
                }
                $status.warning = "MspAgent not provisioned $([math]::Round($waited)) min after restart"
            }
        }

        # Provisioned is not the same as syncing its data. Wait for the inventory components, and restart MspAgent
        # if its log shows the configuration fetch timing out (see Get-UfaSyncState). Bounded: a few restarts, a grace period.
        if ($job.expectUfa -eq 'true' -and $ufaService -and $ufaProvisioned) {
            $coreLog = [Environment]::GetFolderPath('ProgramFilesX86') + '\Msp Agent\logs\msp-agent-core.log'
            $componentLog = [Environment]::GetFolderPath('ProgramFilesX86') + '\Msp Agent\logs\component.log'
            $sync = Get-UfaSyncState $coreLog $since $componentLog
            $status.ufa.sync = $sync.State
            $nowUtc = (Get-Date).ToUniversalTime()
            $restarts = [int]$status.ufaSyncRestarts
            $lastRestart = if ($status.ufaSyncRestartedAt) { [datetime]::Parse($status.ufaSyncRestartedAt).ToUniversalTime() } else { [datetime]::MinValue }
            $stalledDue = $sync.State -eq 'stalled' -and $restarts -lt $UfaSyncMaxRestarts -and ($nowUtc - $lastRestart).TotalMinutes -ge $UfaSyncRestartGapMinutes
            $blockedDue = Test-UfaBlockedRestartDue $sync $restarts $nowUtc $UfaBlockedRestartAfterMinutes $UfaRepairFreshMinutes
            if ($stalledDue -or $blockedDue) {
                Restart-Service -Name 'MspAgent' -Force -ErrorAction SilentlyContinue
                $status.ufaSyncRestarts = $restarts + 1
                $status.ufaSyncRestartedAt = $nowUtc.ToString('o')
                $reason = if ($blockedDue) { "the Ecoverse auth service has not answered the agent since {0:o}" -f $sync.FaultSince } else { "MspAgent configuration fetch timed out at {0:o} and the cloud answers again" -f $sync.FailedAt }
                $limit = if ($blockedDue) { 1 } else { $UfaSyncMaxRestarts }
                Add-StatusHistory $status 'ufa-sync-restart' ("{0}; restarted MspAgent to fetch now instead of at its next hourly check (restart {1} of {2})" -f $reason, ($restarts + 1), $limit)
            }
            $lastRestartNow = if ($status.ufaSyncRestartedAt) { [datetime]::Parse($status.ufaSyncRestartedAt).ToUniversalTime() } else { [datetime]::MinValue }
            if (Test-UfaRepairDue $sync $nowUtc ([bool]$status.ufaRepairedAt) $UfaRepairAfterMinutes $UfaRepairFreshMinutes $UfaBlockedRepairAfterMinutes ([int]$status.ufaSyncRestarts) $lastRestartNow $UfaBlockedSettleMinutes) {
                $result = Invoke-UfaRepair
                # The baseline for watching the Msp Agent folder is the moment the services were restarted.
                $status.ufaRepairedAt = $(if ($script:UfaRepairRestartedAt) { $script:UfaRepairRestartedAt } else { (Get-Date).ToUniversalTime() }).ToString('o')
                Add-StatusHistory $status 'ufa-repair' ("the Ecoverse agent has been in state '{0}' since {1:o} with nothing getting through; removed the UFA core and restarted the Windows Agent services so it is redeployed ({2}); now watching the Msp Agent folder for new data" -f $sync.State, $sync.FaultSince, $result)
                $status.stage = 'awaiting-ufa'
                Save-Status $status
                exit 0
            }
            if ($sync.State -ne 'synced') {
                $waitedSync = ((Get-Date).ToUniversalTime() - $since).TotalMinutes
                # Once a fault has been seen the watcher keeps going for longer: blocked often clears by itself, and a
                # state it turns into (stalled, rejected, denied) needs its own restart or repair clock to run out.
                if ($sync.State -in 'blocked', 'stalled', 'rejected', 'denied', 'nodevice' -and -not $status.ufaFaultSeenAt) { $status.ufaFaultSeenAt = (Get-Date).ToUniversalTime().ToString('o') }
                $grace = if ($status.ufaFaultSeenAt) { $UfaBlockedWatchMinutes } else { $UfaSyncGraceMinutes }
                if ($waitedSync -lt $grace) {
                    $status.stage = 'awaiting-ufa'
                    $why = switch ($sync.State) {
                        'rejected' { 'the cloud rejects the Ecoverse agent login (400 SSL certificate error); a restart does not fix this' }
                        'denied' { 'the component CDN refuses the Ecoverse agent downloads (403); a restart does not fix this' }
                        'blocked' { 'the Ecoverse auth service does not answer the agent; it often clears by itself, so it is watched first' }
                        'nodevice' { 'the Ecoverse agent logs in but the cloud has not given it a device record yet (no GOAT NRI), so its data cannot be sent; normal for about the first 90 minutes after provisioning; after that it needs fixing in the Ecoverse tenant' }
                        default { $sync.State }
                    }
                    Add-StatusHistory $status 'awaiting-ufa' ('MspAgent data sync: {0}; {1:N0} min since install' -f $why, $waitedSync)
                    Save-Status $status
                    exit 0
                }
                $status.warning = if ($sync.State -eq 'nodevice') { "MspAgent logs in but the cloud still has no device record for it (no GOAT NRI), so its data cannot be sent ($([math]::Round($waitedSync)) min after the install)" }
                                  else { "MspAgent had not synced its data $([math]::Round($waitedSync)) min after the install (state: $($sync.State))" }
            }
        }

        $status.stage = 'healthy'
        $status.healthyAt = (Get-Date).ToUniversalTime().ToString('o')
        Add-StatusHistory $status 'healthy' "agent $($agent.Version) on $($agent.Server); UFA present $([bool]$ufaService), provisioned $ufaProvisioned"
        Save-Status $status
        # The token goes now. status.json and the logs stay for the review period, then a one-time task removes the folder.
        Remove-Item $JobPath -Force -ErrorAction SilentlyContinue
        Remove-Item $SetupPath -Force -ErrorAction SilentlyContinue
        $reviewDays = [int]$job.reviewDays
        if ($reviewDays -gt 0) {
            $reviewUntil = (Get-Date).AddDays($reviewDays)
            $command = "Remove-Item -LiteralPath '$Root' -Recurse -Force -ErrorAction SilentlyContinue; if (-not (Get-ChildItem -LiteralPath '$Parent' -Force -ErrorAction SilentlyContinue)) { Remove-Item -LiteralPath '$Parent' -Force -ErrorAction SilentlyContinue }; Unregister-ScheduledTask -TaskName '$ReviewTaskName' -Confirm:`$false"
            $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($command))
            $ps = Join-Path $env:windir 'System32\WindowsPowerShell\v1.0\powershell.exe'
            $action = New-ScheduledTaskAction -Execute $ps -Argument "-NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand $encoded"
            $trigger = New-ScheduledTaskTrigger -Once -At $reviewUntil
            $settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Minutes 30)
            $principal = New-ScheduledTaskPrincipal -UserId 'S-1-5-18' -LogonType ServiceAccount -RunLevel Highest
            Register-ScheduledTask -TaskName $ReviewTaskName -Action $action -Trigger $trigger -Settings $settings -Principal $principal -Description "N-voke: remove migration records after the review period" -Force | Out-Null
            $status.reviewUntil = $reviewUntil.ToUniversalTime().ToString('o')
            Add-StatusHistory $status 'review-period' "status.json and logs kept until $($status.reviewUntil), then removed"
            Save-Status $status
        }
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
        if ($reviewDays -le 0) {
            Remove-Item -LiteralPath $Root -Recurse -Force -ErrorAction SilentlyContinue
            # Launch also creates the parent folder; remove it only if nothing else is in it.
            if (-not (Get-ChildItem -LiteralPath $Parent -Force -ErrorAction SilentlyContinue)) { Remove-Item -LiteralPath $Parent -Force -ErrorAction SilentlyContinue }
        }
        exit 0
    }

    if ($status.stage -eq 'installed-awaiting-health' -and $status.installedAt) {
        $since = (Get-Date).ToUniversalTime() - [datetime]::Parse($status.installedAt).ToUniversalTime()
        if ($since.TotalMinutes -lt $HealthGraceMinutes) {
            Add-StatusHistory $status 'waiting-for-agent' ('{0:N0} min since install; service running: {1}; server: {2}' -f $since.TotalMinutes, $agent.ServiceRunning, $agent.Server)
            Save-Status $status
            exit 0
        }
    }

    if (Get-Process -Name 'WindowsAgentSetup' -ErrorAction SilentlyContinue) {
        $status.stage = 'install-in-progress'
        Add-StatusHistory $status 'install-in-progress' 'an earlier WindowsAgentSetup.exe is still running'
        Save-Status $status
        exit 0
    }
    if (Test-MsiBusy) {
        $status.stage = 'waiting-on-windows'
        Add-StatusHistory $status 'waiting-on-windows' 'Windows Installer is busy (another installation in progress)'
        Save-Status $status
        exit 0
    }
    $status.rebootPending = Test-RebootPending

    $status.stage = 'downloading'
    Save-Status $status
    try {
        Add-StatusHistory $status 'downloaded' (Get-Installer $job.destServer)
    } catch {
        $status.stage = 'download-failed'
        $status.lastError = $_.Exception.Message
        Add-StatusHistory $status 'download-failed' $_.Exception.Message
        Save-Status $status
        exit 1001
    }

    # Everything above leaves the old agent alone. The cleanup and the installer below remove it.
    if ($job.cleanMode -ne 'none' -and -not $status.cleanedAt) {
        $status.stage = 'cleaning'
        Add-StatusHistory $status 'cleanup-started' "mode $($job.cleanMode)"
        Save-Status $status
        Invoke-FullCleanup $status | Out-Null
        $status.cleanedAt = (Get-Date).ToUniversalTime().ToString('o')
        Save-Status $status
    }

    $status.stage = 'installing'
    Add-StatusHistory $status 'install-started' "reboot pending: $($status.rebootPending)"
    Save-Status $status
    $msiLog = Join-Path $LogDir ('msi-{0}.log' -f (Get-Date -Format 'yyyyMMdd-HHmmss'))
    $arguments = '/s /v" /qn SERVERADDRESS={0} CUSTOMERID={1} CUSTOMERSPECIFIC=1 REGISTRATION_TOKEN={2} SERVERPROTOCOL=HTTPS /L*V {3}"' -f $job.destServer, $job.destCustomerId, $job.registrationToken, $msiLog
    $code = (Start-Process -FilePath $SetupPath -ArgumentList $arguments -Wait -PassThru -WindowStyle Hidden).ExitCode
    if ((Hide-RegistrationToken $LogDir $job.registrationToken) -gt 0) { Write-InstallLog 'removed the registration token from the installer log' }

    if ($code -eq 0 -or $code -eq 3010) {
        $status.stage = 'installed-awaiting-health'
        $status.installedAt = (Get-Date).ToUniversalTime().ToString('o')
        $status.lastInstallExitCode = 0
        Add-StatusHistory $status 'install-finished' "exit $code"
        Save-Status $status
        exit 0
    }
    $status.lastInstallExitCode = $code
    if ($code -eq 1618) {
        $status.stage = 'waiting-on-windows'
        Add-StatusHistory $status 'waiting-on-windows' 'installer exit 1618: another installation in progress'
        Save-Status $status
        exit 0
    }
    $status.stage = 'install-failed'
    $status.lastError = "WindowsAgentSetup.exe exit $code"
    Add-StatusHistory $status 'install-failed' "exit $code"
    Save-Status $status
    exit 1003
} catch {
    $status.stage = 'error'
    $status.lastError = $_.Exception.Message
    Add-StatusHistory $status 'error' $_.Exception.Message
    try { Save-Status $status } catch { }
    exit 1005
}
