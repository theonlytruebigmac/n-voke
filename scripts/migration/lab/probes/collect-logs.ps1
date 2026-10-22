<#
.SYNOPSIS
    Collects the logs and facts needed to troubleshoot a migrated device (the Ecoverse agent "MSP Agent", the N-central
    Windows Agent and the N-voke installer) into one zip. Read-only on the device; nothing is changed or restarted.
.DESCRIPTION
    Run on the device in an elevated Windows PowerShell 5.1 or PowerShell 7:
        powershell -NoProfile -ExecutionPolicy Bypass -File collect-logs.ps1
    Add the two N-central servers' names to test the route to them as well:
        powershell -NoProfile -ExecutionPolicy Bypass -File collect-logs.ps1 -ExtraHost nerdstraining.n-able.com,sedemo.focusmsp.net

    Collected (only these; never certificates, vault, database or config files):
      msp-agent\logs and msp-agent\components\*\*\logs   every log of the Ecoverse agent and its components
      msp-agent-programdata\                              the MSI logs in C:\ProgramData\MspAgent\Tools (ufamsi.log ...)
      windows-agent\log                                   the Windows Agent's own logs from the last -Days days (UfaMaint.log ...)
      nvoke\                                              C:\ProgramData\N-voke\Migration: status.json and logs\*.log
      system.txt                                          services, processes, installed products, the Windows Installer
                                                          registration of "MSP Core Agent", proxy, time, scheduled tasks,
                                                          the last 7 days of MsiInstaller / service events, and a DNS +
                                                          TCP + TLS test of the hosts the agent needs (3 tries each)
      listing.txt                                         names, sizes and dates of the folders (no contents)
      manifest.txt                                        what was collected, truncations and how many lines were masked

    Secrets: JWTs, bearer tokens, token/password/secret/key values, URL query secrets and long base64 strings are masked in
    every file. In the Windows Agent and MSI logs, any line that mentions a token, password, secret, key, activation,
    registration, credential or the provisioning GUID is withheld whole. The zip is still worth a glance before it is sent:
    it holds the computer name, IP addresses and Windows user names that appear in the logs.

    Each file is capped at -MaxFileMB (the end of a longer file is kept).
#>
[CmdletBinding()]
param(
    [string]$OutDir = $(if ($env:SystemDrive) { Join-Path $env:SystemDrive 'Temp' } else { [IO.Path]::GetTempPath() }),
    [int]$Days = 3,
    [int]$MaxFileMB = 4,
    [string[]]$ExtraHost = @(),
    [string]$MspAgentDir = ([Environment]::GetFolderPath('ProgramFilesX86') + '\Msp Agent'),
    [string]$MspDataDir = ([Environment]::GetFolderPath('CommonApplicationData') + '\MspAgent'),
    [string]$WindowsAgentDir = ([Environment]::GetFolderPath('ProgramFilesX86') + '\N-able Technologies\Windows Agent'),
    [string]$NvokeDir = ([Environment]::GetFolderPath('CommonApplicationData') + '\N-voke\Migration')
)
$ErrorActionPreference = 'Continue'
$maxBytes = [long]$MaxFileMB * 1MB
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$computer = if ($env:COMPUTERNAME) { $env:COMPUTERNAME } else { [Environment]::MachineName }
$stage = Join-Path ([IO.Path]::GetTempPath()) ("nvoke-logs-$computer-$stamp")
$zipPath = Join-Path $OutDir ("nvoke-logs-$computer-$stamp.zip")
New-Item -ItemType Directory -Path $stage -Force | Out-Null
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null

# ---- masking -------------------------------------------------------------------------------------------------------
$script:Masked = 0
$script:Withheld = 0
$rxJwt = [regex]'eyJ[\w-]{8,}\.[\w-]{8,}\.[\w-]{4,}'
$rxBearer = [regex]'(?i)\b(bearer|basic)\s+[\w.~+/=-]{8,}'
$rxPair = [regex]'(?i)("?[\w.-]*(?:token|secret|passw(?:or)?d|pwd|apikey|api[_-]key|authorization|activation[_-]?key|registration[_-]?key|private[_-]?key|credential)[\w.-]*"?\s*[:=]\s*)("[^"]*"|''[^'']*''|[^\s,;&}]+)'
$rxQuery = [regex]'(?i)([?&](?:token|key|sig|signature|code|password|secret)=)[^&\s]+'
$rxLong = [regex]'[A-Za-z0-9+/]{48,}={0,2}'
$rxStrict = [regex]'(?i)token|secret|passw|authoriz|bearer|activation|registration|credential|api[_-]?key|private[_-]?key|provision\w*\W.*\b(guid|id)\b'

function Hide-Secret([string]$Line, [bool]$Strict) {
    if ($Strict -and $rxStrict.IsMatch($Line)) { $script:Withheld++; return '<line withheld: it may hold a secret>' }
    $out = $rxJwt.Replace($Line, '<jwt>')
    $out = $rxBearer.Replace($out, '$1 <masked>')
    $out = $rxPair.Replace($out, '$1<masked>')
    $out = $rxQuery.Replace($out, '$1<masked>')
    $out = $rxLong.Replace($out, '<base64>')
    if ($out -ne $Line) { $script:Masked++ }
    $out
}

function Read-LogText([string]$Path, [long]$Limit) {
    $fs = [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]'ReadWrite,Delete')
    try {
        $truncated = $fs.Length -gt $Limit
        if ($truncated) { [void]$fs.Seek(-$Limit, [IO.SeekOrigin]::End) }
        $reader = New-Object IO.StreamReader($fs, [Text.Encoding]::UTF8, $true)
        $text = $reader.ReadToEnd()
    }
    finally { $fs.Dispose() }
    if ($truncated) {
        $newline = $text.IndexOf("`n")
        if ($newline -ge 0) { $text = $text.Substring($newline + 1) }
    }
    [pscustomobject]@{ Text = $text; Truncated = $truncated }
}

$manifest = New-Object Collections.Generic.List[string]
function Copy-MaskedLog([IO.FileInfo]$File, [string]$Dest, [bool]$Strict) {
    $target = Join-Path $stage $Dest
    New-Item -ItemType Directory -Path (Split-Path $target -Parent) -Force | Out-Null
    try {
        $read = Read-LogText $File.FullName $maxBytes
        $nul = ($read.Text.ToCharArray() | Where-Object { $_ -eq [char]0 } | Measure-Object).Count
        if ($read.Text.Length -gt 0 -and ($nul / $read.Text.Length) -gt 0.2) {
            $manifest.Add(('{0}  SKIPPED: the file reads as empty (NUL) bytes, usually a copy taken while it was written  <- {1}' -f $Dest, $File.FullName))
            return
        }
        $before = $script:Masked + $script:Withheld
        $lines = New-Object Collections.Generic.List[string]
        foreach ($line in ($read.Text -split "\r?\n")) { $lines.Add((Hide-Secret $line $Strict)) }
        [IO.File]::WriteAllLines($target, [string[]]$lines, (New-Object Text.UTF8Encoding($false)))
        $note = if ($read.Truncated) { "end of the file only (last $MaxFileMB MB)" } else { 'whole file' }
        $manifest.Add(('{0}  {1}; {2} lines changed  <- {3}  ({4:N0} bytes, written {5:yyyy-MM-dd HH:mm:ss})' -f $Dest, $note, ($script:Masked + $script:Withheld - $before), $File.FullName, $File.Length, $File.LastWriteTime))
    }
    catch { $manifest.Add(('{0}  FAILED: {1}  <- {2}' -f $Dest, $_.Exception.Message, $File.FullName)) }
}

function Get-LogFile([string]$Dir, [switch]$Recurse, [datetime]$Newer = [datetime]::MinValue) {
    if (-not (Test-Path -LiteralPath $Dir)) { return @() }
    @(Get-ChildItem -LiteralPath $Dir -File -Recurse:$Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '\.(log|txt)(\.\d+)?$' -and $_.LastWriteTime -ge $Newer })
}

function Get-RelativePath([string]$Root, [string]$Full) { $Full.Substring($Root.TrimEnd('\', '/').Length).TrimStart('\', '/') }

Write-Output "Collecting to $zipPath"
if ($env:OS -eq 'Windows_NT') {
    $admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    if (-not $admin) { Write-Output 'WARNING: not elevated. Some logs and events may be missing. Run this from an elevated PowerShell.' }
}

# ---- the files -----------------------------------------------------------------------------------------------------
$since = (Get-Date).AddDays(-$Days)
foreach ($f in (Get-LogFile (Join-Path $MspAgentDir 'logs'))) { Copy-MaskedLog $f ('msp-agent\logs\' + $f.Name) $false }
foreach ($f in (Get-LogFile (Join-Path $MspAgentDir 'components') -Recurse)) { Copy-MaskedLog $f ('msp-agent\components\' + (Get-RelativePath (Join-Path $MspAgentDir 'components') $f.FullName)) $false }
foreach ($f in (Get-LogFile (Join-Path $MspDataDir 'Tools'))) { Copy-MaskedLog $f ('msp-agent-programdata\Tools\' + $f.Name) $true }
foreach ($f in (Get-LogFile (Join-Path $WindowsAgentDir 'log') -Newer $since)) { Copy-MaskedLog $f ('windows-agent\log\' + $f.Name) $true }
$status = Join-Path $NvokeDir 'status.json'
if (Test-Path -LiteralPath $status) { Copy-MaskedLog (Get-Item -LiteralPath $status) 'nvoke\status.json' $false }
foreach ($f in (Get-LogFile (Join-Path $NvokeDir 'logs'))) { Copy-MaskedLog $f ('nvoke\logs\' + $f.Name) $false }

# ---- facts ---------------------------------------------------------------------------------------------------------
$sys = New-Object Text.StringBuilder
function Add-Section([string]$Title, [scriptblock]$Body) {
    $text = try { (& $Body | Out-String -Width 250).TrimEnd() } catch { 'FAILED: ' + $_.Exception.Message }
    if (-not $text) { $text = '(nothing)' }
    $masked = ($text -split "\r?\n" | ForEach-Object { Hide-Secret $_ $false }) -join "`r`n"
    [void]$sys.AppendLine("==== $Title"); [void]$sys.AppendLine($masked); [void]$sys.AppendLine('')
}

function ConvertTo-PackedGuid([string]$Guid) {
    $h = ($Guid -replace '[{}-]', '').ToUpperInvariant()
    if ($h.Length -ne 32) { return $null }
    $flip = { param($s) $a = $s.ToCharArray(); [array]::Reverse($a); -join $a }
    $packed = (& $flip $h.Substring(0, 8)) + (& $flip $h.Substring(8, 4)) + (& $flip $h.Substring(12, 4))
    for ($i = 16; $i -lt 32; $i += 2) { $packed += $h.Substring($i + 1, 1) + $h.Substring($i, 1) }
    $packed
}

function Test-TlsHost([string]$HostName, [int]$TimeoutMs = 8000) {
    $started = Get-Date
    $result = [ordered]@{ Host = $HostName; Dns = ''; Tcp = ''; Tls = '' }
    try { $result.Dns = ([Net.Dns]::GetHostAddresses($HostName) | ForEach-Object { $_.IPAddressToString }) -join ',' }
    catch { $result.Dns = 'FAILED: ' + $_.Exception.Message; return $result }
    $client = New-Object Net.Sockets.TcpClient
    try {
        $pending = $client.BeginConnect($HostName, 443, $null, $null)
        if (-not $pending.AsyncWaitHandle.WaitOne($TimeoutMs)) { $result.Tcp = 'TIMEOUT'; return $result }
        $client.EndConnect($pending)
        $result.Tcp = 'ok'
        $ssl = New-Object Net.Security.SslStream($client.GetStream(), $false)
        $ssl.ReadTimeout = $TimeoutMs; $ssl.WriteTimeout = $TimeoutMs
        $ssl.AuthenticateAsClient($HostName)
        $cert = New-Object Security.Cryptography.X509Certificates.X509Certificate2($ssl.RemoteCertificate)
        $result.Tls = 'ok {0}; cert {1}; issuer {2}; expires {3:yyyy-MM-dd}' -f $ssl.SslProtocol, $cert.Subject, $cert.Issuer, $cert.NotAfter
    }
    catch {
        $inner = $_.Exception
        while ($inner.InnerException) { $inner = $inner.InnerException }
        if ($result.Tcp -eq 'ok') { $result.Tls = 'FAILED: ' + $inner.Message } else { $result.Tcp = 'FAILED: ' + $inner.Message }
    }
    finally { $client.Close(); $result.Ms = [int]((Get-Date) - $started).TotalMilliseconds }
    $result
}

Add-Section 'Collection' {
    "Collected at: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss zzz') (UTC $([DateTime]::UtcNow.ToString('yyyy-MM-dd HH:mm:ss')))"
    "Computer: $computer    Run as: $(whoami)    PowerShell: $($PSVersionTable.PSVersion)"
    "Windows logs kept from the last $Days days; each file capped at $MaxFileMB MB"
}
Add-Section 'Operating system and time' {
    Get-CimInstance Win32_OperatingSystem | Select-Object Caption, Version, BuildNumber, LastBootUpTime | Format-List
    (Get-TimeZone | Select-Object Id, DisplayName, BaseUtcOffset | Format-List)
    w32tm /query /status 2>&1
}
Add-Section 'Services (Msp / N-able / Windows Agent)' {
    Get-CimInstance Win32_Service | Where-Object { $_.Name -match 'msp|windows ?agent|n-?able|ncentral|probe' -or $_.DisplayName -match 'msp|windows agent|n-able|n-central' } |
        Select-Object Name, DisplayName, State, StartMode, ProcessId, StartName, PathName | Format-List
}
Add-Section 'Processes' {
    $pattern = 'msp|n-?able|agentmaint|interrogator|msiexec|ufa|ncentral|nvoke|^agent$|windowsagent|basupsrv'
    Get-Process | ForEach-Object {
        $path = try { $_.Path } catch { $null }
        if ($_.ProcessName -match $pattern -or $path -match 'Msp Agent|Windows Agent|N-able') {
            [pscustomobject]@{ Name = $_.ProcessName; Id = $_.Id; Started = $_.StartTime; Responding = $_.Responding; Path = $path }
        }
    } | Format-Table -AutoSize
}
Add-Section 'Installed products (Uninstall keys)' {
    $keys = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*', 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*'
    Get-ItemProperty $keys -ErrorAction SilentlyContinue | Where-Object { $_.DisplayName -match 'msp|windows agent|n-able|n-central' } |
        Select-Object DisplayName, DisplayVersion, Publisher, PSChildName, UninstallString | Format-List
}
Add-Section 'Windows Installer registration of "MSP Core Agent" (present or absent)' {
    $codes = @('{A14A5D24-6709-4571-A0E4-1769230C3267}')
    $found = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*', 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*' -ErrorAction SilentlyContinue |
        Where-Object { $_.DisplayName -eq 'MSP Core Agent' -and $_.PSChildName -match '^\{[0-9A-Fa-f-]{36}\}$' }
    foreach ($entry in $found) { if ($codes -notcontains $entry.PSChildName) { $codes += $entry.PSChildName } }
    foreach ($code in $codes) {
        $p = ConvertTo-PackedGuid $code
        "Product $code (packed $p)"
        foreach ($path in "HKLM:\SOFTWARE\Classes\Installer\Products\$p", "HKLM:\SOFTWARE\Classes\Installer\Features\$p", "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Installer\UserData\S-1-5-18\Products\$p") {
            '  {0,-8} {1}' -f $(if (Test-Path -LiteralPath $path) { 'present' } else { 'absent' }), $path
        }
    }
    $upgrade = ConvertTo-PackedGuid '{B28B3FA8-9310-4AA7-B0CA-CBCCA5FD00B2}'
    $path = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Installer\UpgradeCodes\$upgrade"
    'Upgrade code (packed {0}): {1}' -f $upgrade, $(if (Test-Path -LiteralPath $path) { 'present' } else { 'absent' })
    'msp-agent-core.exe: {0}' -f $(if (Test-Path -LiteralPath (Join-Path $MspAgentDir 'msp-agent-core.exe')) { 'present' } else { 'absent' })
}
Add-Section 'Proxy' {
    netsh winhttp show proxy 2>&1
    Get-ItemProperty 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Internet Settings' -ErrorAction SilentlyContinue | Select-Object ProxyEnable, ProxyServer, AutoConfigURL | Format-List
}
Add-Section 'Scheduled tasks (N-voke / migration)' {
    Get-ScheduledTask -ErrorAction SilentlyContinue | Where-Object { $_.TaskName -match 'n-?voke|migration' } | ForEach-Object {
        $info = $_ | Get-ScheduledTaskInfo
        '{0}  state {1}  last run {2}  last result {3}  next run {4}' -f $_.TaskName, $_.State, $info.LastRunTime, $info.LastTaskResult, $info.NextRunTime
    }
}
Add-Section 'Events: MsiInstaller (last 7 days, Msp / N-able only)' {
    Get-WinEvent -FilterHashtable @{ LogName = 'Application'; ProviderName = 'MsiInstaller'; StartTime = (Get-Date).AddDays(-7) } -ErrorAction SilentlyContinue |
        Where-Object { $_.Message -match 'MSP Core Agent|Windows Agent|N-able|Msp Agent' } | Sort-Object TimeCreated |
        ForEach-Object { '{0:yyyy-MM-dd HH:mm:ss}  {1,5}  {2}' -f $_.TimeCreated, $_.Id, (($_.Message -replace '\s+', ' ').Substring(0, [Math]::Min(300, ($_.Message -replace '\s+', ' ').Length))) }
}
Add-Section 'Events: services and crashes (last 7 days, Msp / N-able only)' {
    $names = @(
        @{ LogName = 'System'; ProviderName = 'Service Control Manager'; StartTime = (Get-Date).AddDays(-7) },
        @{ LogName = 'Application'; ProviderName = 'Application Error'; StartTime = (Get-Date).AddDays(-7) })
    foreach ($filter in $names) {
        Get-WinEvent -FilterHashtable $filter -ErrorAction SilentlyContinue | Where-Object { $_.Message -match 'msp|windows agent|n-able|agentmaintenance' } | Sort-Object TimeCreated |
            ForEach-Object { '{0:yyyy-MM-dd HH:mm:ss}  {1,5}  {2}' -f $_.TimeCreated, $_.Id, (($_.Message -replace '\s+', ' ').Substring(0, [Math]::Min(300, ($_.Message -replace '\s+', ' ').Length))) }
    }
}
$hosts = @('auth.us-west-2.prd.cdo.system-monitor.com', 'configs.us-west-2.prd.cdo.system-monitor.com', 'provisioning.config.us-west-2.prd.cdo.system-monitor.com',
    'ingest.us-west-2.prd.cdo.system-monitor.com', 'cdn-component.fusion.prd.cdo.system-monitor.com', 'cdn-notary.pub.prd.cdo.system-monitor.com') + @($ExtraHost | Where-Object { $_ })
Write-Output "Testing the route to $($hosts.Count) hosts (3 tries each)..."
Add-Section 'Network: DNS, TCP 443 and TLS handshake without a client certificate (3 tries, 2 s apart)' {
    'A handshake that is reset or closed by an agent host (auth, ingest) is expected here: those hosts want the agent''s client certificate. A DNS or TCP failure, a long delay or an unexpected certificate issuer (a proxy or firewall in the middle) is the finding.'
    ''
    foreach ($name in $hosts) {
        foreach ($try in 1..3) {
            $r = Test-TlsHost $name
            '{0}  try {1}: dns [{2}]  tcp [{3}]  tls [{4}]  {5} ms' -f $name, $try, $r.Dns, $r.Tcp, $r.Tls, $r.Ms
            if ($try -lt 3) { Start-Sleep -Seconds 2 }
        }
    }
}
[IO.File]::WriteAllText((Join-Path $stage 'system.txt'), $sys.ToString(), (New-Object Text.UTF8Encoding($false)))

# ---- listings (names only) -----------------------------------------------------------------------------------------
$listing = New-Object Text.StringBuilder
foreach ($dir in @($MspAgentDir, $MspDataDir, $WindowsAgentDir, $NvokeDir)) {
    [void]$listing.AppendLine("==== $dir")
    if (Test-Path -LiteralPath $dir) {
        $rows = Get-ChildItem -LiteralPath $dir -Recurse -Depth 3 -Force -ErrorAction SilentlyContinue | Select-Object @{ n = 'Path'; e = { $_.FullName.Substring($dir.Length) } }, Length, LastWriteTime
        [void]$listing.AppendLine(($rows | Format-Table -AutoSize | Out-String -Width 250).TrimEnd())
    }
    else { [void]$listing.AppendLine('(not there)') }
    [void]$listing.AppendLine('')
}
[IO.File]::WriteAllText((Join-Path $stage 'listing.txt'), $listing.ToString(), (New-Object Text.UTF8Encoding($false)))

# ---- manifest and zip ----------------------------------------------------------------------------------------------
$head = @("n-voke log collection from $computer at $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss zzz')",
    "Lines masked: $script:Masked    Lines withheld whole: $script:Withheld", "Files collected: $($manifest.Count)", '')
[IO.File]::WriteAllLines((Join-Path $stage 'manifest.txt'), [string[]]($head + $manifest), (New-Object Text.UTF8Encoding($false)))
if (Test-Path -LiteralPath $zipPath) { Remove-Item -LiteralPath $zipPath -Force }
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zipPath -Force
Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
$size = (Get-Item -LiteralPath $zipPath).Length
Write-Output ''
Write-Output ("Done: {0}  ({1:N1} MB, {2} files, {3} lines masked, {4} withheld)" -f $zipPath, ($size / 1MB), $manifest.Count, $script:Masked, $script:Withheld)
Write-Output 'Open the zip and glance through it before you send it. manifest.txt lists everything in it.'
