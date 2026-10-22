<#
.SYNOPSIS
    Checks the Ecoverse agent (UFA, "MSP Agent") on a device, in the order a device gets there, and says how far it got,
    what is wrong and what to do. Read-only. Prints no secrets.
.DESCRIPTION
    Run on the device, in an elevated Windows PowerShell 5.1 or PowerShell 7:
        powershell -NoProfile -ExecutionPolicy Bypass -File ufa-log-summary.ps1
    Add -Network to test the route to the agent's hosts, -Full for every detail, -OutFile C:\temp\check.txt to save it.

    To check a device from what collect-logs.ps1 gathered (a zip, or the folder it was extracted to), on any machine:
        powershell -NoProfile -File ufa-log-summary.ps1 -Collected C:\temp\nvoke-logs-SUP-2190-20261022-140311.zip
    The time of the collection is read from the zip, so "30 minutes ago" means 30 minutes before it was taken.
    For a lone copy of the core log: -LogPath C:\temp\msp-agent-core.log (and -AsOfUtc 2026-10-22T18:05:00Z, the time the
    copy was taken, since only errors from the last 30 minutes count for the "no device record" state).

    The report: RESULT and what to do first; then one line per stage, from the Windows Agent to the data leaving the
    device ([ ok ] fine, [WARN] look, [FAIL] broken, [info] for your information, [ -- ] could not be checked); then the
    evidence behind every line that is not fine, the installer's own record, a timeline and the errors nobody expected.

    What it can't see: Ecoverse itself. "No fault found on the device" is not "Ecoverse has the data". The Track step
    in N-voke (Ecoverse data received) is the check for that.

    Times are the device's local time as written in its logs.
#>
[CmdletBinding()]
param(
    [string]$LogPath = ([Environment]::GetFolderPath('ProgramFilesX86') + '\Msp Agent\logs\msp-agent-core.log'),
    [string]$ComponentLogPath = (Join-Path (Split-Path $LogPath -Parent) 'component.log'),
    [string]$Collected,
    [string]$OutFile,
    [string]$AsOfUtc,
    [switch]$Network,
    [switch]$Full
)
$ErrorActionPreference = 'Continue'
$wantNetwork = $Network.IsPresent

# BEGIN shared with endpoint/installAgent.ps1. This is a verbatim copy of its Get-UfaSyncState, so the STATE below is the
# one the installer's watcher acts on. Endpoint.Tests.ps1 fails if the two copies differ: change it there, then copy it here.
function Get-UfaSyncState([string]$LogPath, [datetime]$Since, [string]$ComponentLogPath = '', [datetime]$NowUtc = [datetime]::UtcNow) {
    if (-not (Test-Path -LiteralPath $LogPath)) { return [pscustomobject]@{ State = 'unknown'; FailedAt = $null; RejectedSince = $null; FaultSince = $null; FaultLastAt = $null } }
    $stamp = [regex]'^\[(\d{4}-\d\d-\d\dT[\d:.]+[+-]\d\d:\d\d)\]'
    $failedAt = $null; $failingSince = $null; $jwtFailedAt = $null; $authOkAt = $null; $syncedAt = $null; $rejectedAt = $null; $rejectedSince = $null; $deniedCount = 0; $deniedSince = $null; $deniedAt = $null; $recordFoundAt = $null
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
        elseif ($line -match 'configservice::updateDeviceConfig device GOAT NRI was found') { $recordFoundAt = $at }
        elseif ($line -match 'components::Download Successfully downloaded') {
            $deniedCount = 0; $deniedSince = $null
            if ($line -match 'generic-asset-interrogator') { $syncedAt = $at }
        }
    }
    # The components' own log: each flush of their data starts by asking for the device's identity and, without one, stops.
    # And the proof that data leaves the device: the batch of inventory (DeviceAsset) the messaging layer logs as sent.
    $noDeviceTimes = New-Object Collections.Generic.List[datetime]; $inventorySentAt = $null; $componentLogRead = $false
    if ($ComponentLogPath -and (Test-Path -LiteralPath $ComponentLogPath)) {
        $componentLogRead = $true
        $windowStart = $NowUtc.AddMinutes(-30)
        foreach ($line in @(Get-Content -LiteralPath $ComponentLogPath -Tail 5000 -ErrorAction SilentlyContinue)) {
            $sentInventory = $line -match 'processMessages .*sending \d+ messages for typeURL: \S+DeviceAsset\s*$'
            if (-not $sentInventory -and $line -notmatch 'ERROR \(\d+\): messaging::refreshDeviceInfo failed to get goat NRI') { continue }
            $m = $stamp.Match($line)
            if (-not $m.Success) { continue }
            $at = [datetimeoffset]::Parse($m.Groups[1].Value, [Globalization.CultureInfo]::InvariantCulture).UtcDateTime
            if ($at -lt $Since) { continue }
            if ($sentInventory) { $inventorySentAt = $at; continue }
            if ($at -ge $windowStart) { $noDeviceTimes.Add($at) }
        }
    }
    # An error from before the record arrived says nothing about now.
    $noDeviceNow = @($noDeviceTimes | Where-Object { -not $recordFoundAt -or $_ -gt $recordFoundAt })
    $noDeviceCount = $noDeviceNow.Count
    $noDeviceSince = if ($noDeviceCount) { $noDeviceNow[0] } else { $null }
    $noDeviceAt = if ($noDeviceCount) { $noDeviceNow[$noDeviceCount - 1] } else { $null }
    # With the components' log to read, "synced" is proof: the device record arrived or the inventory was sent. Without it,
    # the inventory component having been downloaded is all there is to go on.
    $proofAt = if ($recordFoundAt -and $inventorySentAt) { if ($recordFoundAt -gt $inventorySentAt) { $recordFoundAt } else { $inventorySentAt } } elseif ($recordFoundAt) { $recordFoundAt } else { $inventorySentAt }
    $isSynced = if ($componentLogRead) { $proofAt -and (-not $failedAt -or $proofAt -gt $failedAt) } else { $syncedAt -and (-not $failedAt -or $syncedAt -gt $failedAt) }
    $state = if ($noDeviceCount -ge 2) { 'nodevice' }
             elseif ($isSynced) { 'synced' }
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
# END shared

# ---- where things are ----------------------------------------------------------------------------------------------
$cleanupRoot = $null
$root = $null
if ($Collected) {
    $root = $Collected
    if ($Collected -match '\.zip$') {
        $root = Join-Path ([IO.Path]::GetTempPath()) ('nvoke-check-' + [guid]::NewGuid().ToString('N'))
        Expand-Archive -LiteralPath $Collected -DestinationPath $root -Force
        $cleanupRoot = $root
    }
    $LogPath = Join-Path $root 'msp-agent/logs/msp-agent-core.log'
    $ComponentLogPath = Join-Path $root 'msp-agent/logs/component.log'
}
$programFiles = [Environment]::GetFolderPath('ProgramFilesX86')
$programData = [Environment]::GetFolderPath('CommonApplicationData')
$src = @{
    Logs       = Split-Path $LogPath -Parent
    Components = if ($root) { Join-Path $root 'msp-agent/components' } else { Join-Path (Split-Path (Split-Path $LogPath -Parent) -Parent) 'components' }
    AgentDir   = if ($root) { $null } else { Split-Path (Split-Path $LogPath -Parent) -Parent }
    WindowsLog = if ($root) { Join-Path $root 'windows-agent/log' } else { $programFiles + '\N-able Technologies\Windows Agent\log' }
    Nvoke      = if ($root) { Join-Path $root 'nvoke' } else { $programData + '\N-voke\Migration' }
    Tools      = if ($root) { Join-Path $root 'msp-agent-programdata/Tools' } else { $programData + '\MspAgent\Tools' }
    System     = if ($root) { Join-Path $root 'system.txt' } else { $null }
}
$systemText = if ($src.System -and (Test-Path -LiteralPath $src.System)) { (Get-Content -LiteralPath $src.System -Raw) -replace "`r`n", "`n" } else { '' }

$utcStyle = [Globalization.DateTimeStyles]'AssumeUniversal,AdjustToUniversal'
$nowUtc = [datetime]::UtcNow
if ($AsOfUtc) { $nowUtc = [datetime]::Parse($AsOfUtc, [Globalization.CultureInfo]::InvariantCulture, $utcStyle) }
elseif ($systemText -match 'Collected at: .*\(UTC (\d{4}-\d\d-\d\d \d\d:\d\d:\d\d)\)') { $nowUtc = [datetime]::Parse($Matches[1], [Globalization.CultureInfo]::InvariantCulture, $utcStyle) }

# ---- reading logs --------------------------------------------------------------------------------------------------
$script:Offset = [TimeZoneInfo]::Local.GetUtcOffset($nowUtc)
$stampRx = [regex]'^\[(\d{4}-\d\d-\d\dT[\d:.]+[+-]\d\d:\d\d)\]\s+([A-Z]+)\s+\(\d+\):\s?(.*)$'

function Read-LogLine([string]$Path) {
    if (-not $Path -or -not (Test-Path -LiteralPath $Path -PathType Leaf)) { return @() }
    try {
        $fs = [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]'ReadWrite,Delete')
        try { $text = (New-Object IO.StreamReader($fs, [Text.Encoding]::UTF8, $true)).ReadToEnd() } finally { $fs.Dispose() }
    }
    catch { return @() }
    @($text -split "\r?\n")
}

function Read-StampedLog([string]$Path) {
    $rows = New-Object Collections.Generic.List[object]
    foreach ($line in (Read-LogLine $Path)) {
        $m = $stampRx.Match($line)
        if (-not $m.Success) { continue }
        $dto = [datetimeoffset]::Parse($m.Groups[1].Value, [Globalization.CultureInfo]::InvariantCulture)
        $script:Offset = $dto.Offset
        $rows.Add([pscustomobject]@{ At = $dto.UtcDateTime; Level = $m.Groups[2].Value; Text = $m.Groups[3].Value })
    }
    $rows.ToArray()
}

$core = @(Read-StampedLog $LogPath)
$comp = @(Read-StampedLog $ComponentLogPath)
$script:NowLocal = $nowUtc + $script:Offset

function Format-Clock($Utc) {
    if (-not $Utc) { return '-' }
    $local = ([datetime]$Utc) + $script:Offset
    if ($local.Date -eq $script:NowLocal.Date) { $local.ToString('HH:mm:ss') } else { $local.ToString('MM-dd HH:mm') }
}
function Format-Age($Utc) {
    if (-not $Utc) { return '-' }
    $minutes = [int](($nowUtc - [datetime]$Utc).TotalMinutes)
    if ($minutes -lt 1) { 'just now' } elseif ($minutes -lt 90) { "$minutes min ago" } elseif ($minutes -lt 2880) { '{0:N1} h ago' -f ($minutes / 60) } else { '{0:N0} days ago' -f ($minutes / 1440) }
}
function Format-Duration([timespan]$Span) {
    if ($Span.TotalMinutes -lt 1.5) { '{0:N0} s' -f $Span.TotalSeconds } elseif ($Span.TotalMinutes -lt 90) { '{0:N0} min' -f $Span.TotalMinutes } else { '{0:N1} h' -f $Span.TotalHours }
}
function Convert-LocalToUtc([datetime]$Local) { $Local - $script:Offset }

# ---- what the system says (live, or the collector's system.txt) -----------------------------------------------------
function Get-SystemSection([string]$Title) {
    $m = [regex]::Match($systemText, '(?ms)^==== ' + [regex]::Escape($Title) + '[^\r\n]*\r?\n(.*?)(?=^==== |\z)')
    if ($m.Success) { $m.Groups[1].Value } else { '' }
}

# Each fact is $null when it could not be read (so a check can say "could not be checked").
function Get-ServiceFact {
    $found = @{}
    if ($root) {
        $text = Get-SystemSection 'Services'
        if (-not $text) { return $null }
        foreach ($m in [regex]::Matches($text, '(?ms)^Name\s*:\s*(?<n>.+?)\r?\n^DisplayName\s*:\s*(?<d>.+?)\r?\n^State\s*:\s*(?<s>\w+)')) { $found[$m.Groups['n'].Value.Trim()] = $m.Groups['s'].Value }
        return $found
    }
    try {
        foreach ($s in @(Get-CimInstance Win32_Service -ErrorAction Stop)) { if ($s.Name -match 'MspAgent|Windows Agent') { $found[$s.Name] = [string]$s.State } }
        $found
    }
    catch { $null }
}
function Get-ProductFact {
    $list = New-Object Collections.Generic.List[object]
    if ($root) {
        $text = Get-SystemSection 'Installed products'
        if (-not $text) { return $null }
        foreach ($m in [regex]::Matches($text, '(?ms)^DisplayName\s*:\s*(?<n>.+?)\r?\n^DisplayVersion\s*:\s*(?<v>.*?)\r?\n')) { $list.Add([pscustomobject]@{ Name = $m.Groups['n'].Value.Trim(); Version = $m.Groups['v'].Value.Trim() }) }
        return $list.ToArray()
    }
    try {
        $keys = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*', 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*'
        foreach ($p in @(Get-ItemProperty $keys -ErrorAction SilentlyContinue | Where-Object { $_.DisplayName -match 'MSP Core Agent|Windows Agent' })) { $list.Add([pscustomobject]@{ Name = $p.DisplayName; Version = $p.DisplayVersion }) }
        $list.ToArray()
    }
    catch { $null }
}
# present / absent for: the Windows Installer registration of the MSP Core Agent, and its program file
function Get-RegistrationFact {
    if ($root) {
        $text = Get-SystemSection 'Windows Installer registration'
        if (-not $text) { return $null }
        $parts = @([regex]::Matches($text, '(?m)^\s+(present|absent)\s+HKLM') | ForEach-Object { $_.Groups[1].Value })
        $exe = [regex]::Match($text, 'msp-agent-core\.exe:\s*(present|absent)')
        return [pscustomobject]@{ Keys = $parts; Exe = $(if ($exe.Success) { $exe.Groups[1].Value } else { $null }) }
    }
    try {
        $packed = '42D5A41A907617540A4E719632C02376'
        $keys = @("HKLM:\SOFTWARE\Classes\Installer\Products\$packed", "HKLM:\SOFTWARE\Classes\Installer\Features\$packed", "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Installer\UserData\S-1-5-18\Products\$packed")
        $parts = @($keys | ForEach-Object { if (Test-Path -LiteralPath $_) { 'present' } else { 'absent' } })
        $exePath = if ($src.AgentDir) { Join-Path $src.AgentDir 'msp-agent-core.exe' } else { $null }
        [pscustomobject]@{ Keys = $parts; Exe = $(if ($exePath -and (Test-Path -LiteralPath $exePath)) { 'present' } else { 'absent' }) }
    }
    catch { $null }
}
function Get-InstallerEventFact {
    $events = New-Object Collections.Generic.List[object]
    if ($root) {
        $text = Get-SystemSection 'Events: MsiInstaller'
        if (-not $text) { return $null }
        foreach ($m in [regex]::Matches($text, '(?m)^(\d{4}-\d\d-\d\d \d\d:\d\d:\d\d)\s+(\d+)\s+(.*)$')) {
            if ($m.Groups[3].Value -notmatch 'MSP Core Agent') { continue }
            $events.Add([pscustomobject]@{ At = Convert-LocalToUtc ([datetime]::Parse($m.Groups[1].Value, [Globalization.CultureInfo]::InvariantCulture)); Id = [int]$m.Groups[2].Value; Message = $m.Groups[3].Value })
        }
        return $events.ToArray()
    }
    try {
        $filter = @{ LogName = 'Application'; ProviderName = 'MsiInstaller'; StartTime = (Get-Date).AddDays(-7) }
        foreach ($e in @(Get-WinEvent -FilterHashtable $filter -ErrorAction Stop | Where-Object { $_.Message -match 'MSP Core Agent' } | Sort-Object TimeCreated)) {
            $events.Add([pscustomobject]@{ At = $e.TimeCreated.ToUniversalTime(); Id = $e.Id; Message = ($e.Message -replace '\s+', ' ') })
        }
        $events.ToArray()
    }
    catch { $null }
}
function Get-ClockFact {
    if ($root) {
        $text = Get-SystemSection 'Operating system and time'
        if (-not $text) { return $null }
        $source = [regex]::Match($text, '(?m)^Source:\s*(.+)$')
        $leap = [regex]::Match($text, '(?m)^Leap Indicator:\s*(.+)$')
        if (-not $source.Success) { return $null }
        return [pscustomobject]@{ Source = $source.Groups[1].Value.Trim(); Synced = ($leap.Success -and $leap.Groups[1].Value -notmatch 'not synchronized') }
    }
    try {
        $out = @(w32tm /query /status 2>&1) -join "`n"
        $source = [regex]::Match($out, '(?m)^Source:\s*(.+)$')
        if (-not $source.Success) { $script:ClockNote = ($out -split "\r?\n" | Where-Object { $_.Trim() } | Select-Object -First 1); return $null }
        [pscustomobject]@{ Source = $source.Groups[1].Value.Trim(); Synced = ($out -notmatch 'not synchronized') }
    }
    catch { $null }
}
function Get-NetworkFact {
    $rows = New-Object Collections.Generic.List[object]
    if ($root) {
        $text = Get-SystemSection 'Network'
        if (-not $text) { return $null }
        foreach ($m in [regex]::Matches($text, '(?m)^(?<h>\S+)\s+try \d+: dns \[(?<dns>[^\]]*)\]\s+tcp \[(?<tcp>[^\]]*)\].*?(?<ms>\d+) ms$')) {
            $rows.Add([pscustomobject]@{ Host = $m.Groups['h'].Value; Ok = ($m.Groups['tcp'].Value -eq 'ok'); Why = $m.Groups['tcp'].Value; Ms = [int]$m.Groups['ms'].Value })
        }
        if ($rows.Count -eq 0) { return $null }
        return $rows.ToArray()
    }
    if (-not $wantNetwork) { return $null }
    $hosts = 'auth.us-west-2.prd.cdo.system-monitor.com', 'configs.us-west-2.prd.cdo.system-monitor.com', 'provisioning.config.us-west-2.prd.cdo.system-monitor.com', 'ingest.us-west-2.prd.cdo.system-monitor.com', 'cdn-component.fusion.prd.cdo.system-monitor.com', 'cdn-notary.pub.prd.cdo.system-monitor.com'
    foreach ($name in $hosts) {
        $started = Get-Date
        $client = New-Object Net.Sockets.TcpClient
        try {
            $pending = $client.BeginConnect($name, 443, $null, $null)
            $ok = $pending.AsyncWaitHandle.WaitOne(4000)
            if ($ok) { $client.EndConnect($pending) }
            $rows.Add([pscustomobject]@{ Host = $name; Ok = $ok; Why = $(if ($ok) { 'ok' } else { 'no answer in 4 s' }); Ms = [int]((Get-Date) - $started).TotalMilliseconds })
        }
        catch { $rows.Add([pscustomobject]@{ Host = $name; Ok = $false; Why = $_.Exception.Message; Ms = 0 }) }
        finally { $client.Close() }
    }
    $rows.ToArray()
}

# ---- the checks ----------------------------------------------------------------------------------------------------
$checks = New-Object Collections.Generic.List[object]
function Add-Check([string]$Name, [string]$Status, [string]$Summary, [string[]]$Detail = @()) {
    $checks.Add([pscustomobject]@{ Name = $Name; Status = $Status; Summary = $Summary; Detail = @($Detail) })
}
$mark = @{ ok = '[ ok ]'; warn = '[WARN]'; fail = '[FAIL]'; info = '[info]'; skip = '[ -- ]' }

$sync = Get-UfaSyncState $LogPath ([datetime]::MinValue) $ComponentLogPath $nowUtc
$installer = $null
$statusPath = Join-Path $src.Nvoke 'status.json'
if (Test-Path -LiteralPath $statusPath) { try { $installer = (Read-LogLine $statusPath) -join "`n" | ConvertFrom-Json } catch { $installer = $null } }

# Facts from the core log, in one pass.
$install = if ($core.Count) { $core[0].At } else { $null }
$prov = @{}
$oauth = New-Object Collections.Generic.List[object]
$serviceStarts = New-Object Collections.Generic.List[datetime]
$noConfig = New-Object Collections.Generic.List[datetime]
$noGoat = New-Object Collections.Generic.List[datetime]
$goatOther = New-Object Collections.Generic.List[string]
$goatFound = New-Object Collections.Generic.List[datetime]
$downloads = New-Object Collections.Generic.List[object]
$denied = 0
$deniedLast = $null
$upgrades = 0
$configOk = New-Object Collections.Generic.List[datetime]
$configFail = New-Object Collections.Generic.List[datetime]
foreach ($r in $core) {
    $t = $r.Text
    if ($t -match 'AgentIsProvisioned current product provisioning state: \[ (\d) \]') { if (-not $prov.ContainsKey($Matches[1])) { $prov[$Matches[1]] = $r.At } }
    elseif ($t -match 'requestOauthToken Oauth token response received\. Status: (\d+)') { if ($Matches[1] -eq '200') { $oauth.Add([pscustomobject]@{ At = $r.At; Result = '200' }) } elseif ($Matches[1] -ne '400') { $oauth.Add([pscustomobject]@{ At = $r.At; Result = $Matches[1] }) } }
    elseif ($t -match 'fetchJWTWithRetries error occurred when fetching jwt: unexpected status code: 400 .*The SSL certificate error') { $oauth.Add([pscustomobject]@{ At = $r.At; Result = 'ssl400' }) }
    elseif ($t -match 'requestOauthToken unable to get jwt: .*Client\.Timeout') { $oauth.Add([pscustomobject]@{ At = $r.At; Result = 'timeout' }) }
    elseif ($t -match 'requestOauthToken unable to get jwt: .*(forcibly closed|connection reset)') { $oauth.Add([pscustomobject]@{ At = $r.At; Result = 'reset' }) }
    elseif ($t -match 'main::run Msp Agent Service starting') { $serviceStarts.Add($r.At) }
    elseif ($t -match 'updateDeviceConfig no device configuration found') { $noConfig.Add($r.At) }
    elseif ($t -match 'updateDeviceConfig no device GOAT NRI was found') { $noGoat.Add($r.At) }
    elseif ($t -match 'updateDeviceConfig device GOAT NRI was found') { $goatFound.Add($r.At) }
    elseif ($t -match '(?i)goat' -and $t -notmatch 'goatNri element not found') { $goatOther.Add($t) }
    elseif ($t -match 'components::Download Successfully downloaded .*components/([\w-]+)/') { $downloads.Add([pscustomobject]@{ At = $r.At; Name = $Matches[1] }) }
    elseif ($t -match 'doRequestWithRetry received invalid status: ''403''') { $denied++; $deniedLast = $r.At }
    elseif ($t -match 'Upgrade is required: \[ true \]') { $upgrades++ }
    elseif ($t -match 'fetchConfigurationsAndSave response received\. Status: 200') { $configOk.Add($r.At) }
    elseif ($t -match 'FetchAllConfigurations failed to fetch configuration data') { $configFail.Add($r.At) }
}
$nri = @($comp | Where-Object { $_.Text -match 'refreshDeviceInfo failed to get goat NRI' })
$flushes = @($comp | Where-Object { $_.Text -match 'processMessage .*flush requested by component' })
# Proof that data leaves the device: the messaging layer logs each batch it sends, by message type. DeviceAsset is the inventory.
$sent = @($comp | Where-Object { $_.Text -match 'processMessages .*sending \d+ messages for typeURL: \S+\.(\w+)$' } | ForEach-Object { [pscustomobject]@{ At = $_.At; Type = $Matches[1] } })
$sentFailed = @($comp | Where-Object { $_.Text -match 'processMessages .*failed to send \d+ messages for typeURL: \S+\.(\w+): (.*)$' } | ForEach-Object { [pscustomobject]@{ At = $_.At; Type = $Matches[1]; Why = $Matches[2] } })
$inventorySent = @($sent | Where-Object { $_.Type -eq 'DeviceAsset' })
$reacquired = @($comp | Where-Object { $_.Text -match 'failed integrity check, re-acquiring' }).Count

$services = Get-ServiceFact
$products = Get-ProductFact
$registration = Get-RegistrationFact
$msiEvents = Get-InstallerEventFact
$clock = Get-ClockFact
$net = Get-NetworkFact

# Windows Agent log: the maintenance that deploys the Ecoverse agent. Its times are the device's local time, with no offset.
$ufaMaint = New-Object Collections.Generic.List[object]
$ufaRx = [regex]'^\[[^\]]*\]\s+(\d{4}-\d\d-\d\d \d\d:\d\d:\d\d),\d+\s+\S+\s+\S+\s+(.*)$'
foreach ($line in (Read-LogLine (Join-Path $src.WindowsLog 'UfaMaint.log'))) {
    $m = $ufaRx.Match($line)
    if ($m.Success) { $ufaMaint.Add([pscustomobject]@{ At = Convert-LocalToUtc ([datetime]::Parse($m.Groups[1].Value, [Globalization.CultureInfo]::InvariantCulture)); Text = $m.Groups[2].Value }) }
}

# 1. Windows Agent
if ($services) {
    $wa = @($services.Keys | Where-Object { $_ -match 'Windows Agent' })
    $notRunning = @($wa | Where-Object { $services[$_] -ne 'Running' })
    $version = @($products | Where-Object { $_.Name -eq 'Windows Agent' } | ForEach-Object { $_.Version })
    $server = if ($installer -and $installer.agent -and $installer.agent.server) { [string]$installer.agent.server } else { $null }
    $line = 'v{0}{1}' -f $(if ($version) { $version[0] } else { '?' }), $(if ($server) { ", reporting to $server" } else { '' })
    if (-not $wa.Count) { Add-Check 'Windows Agent' 'fail' 'not installed: the Windows Agent is what deploys the Ecoverse agent' }
    elseif ($notRunning) { Add-Check 'Windows Agent' 'fail' ('{0} not running' -f ($notRunning -join ', ')) @("The Windows Agent deploys and repairs the Ecoverse agent; with it stopped nothing happens. $line") }
    else { Add-Check 'Windows Agent' 'ok' "running, $line" }
}
else { Add-Check 'Windows Agent' 'skip' 'could not read the services' }

# 2. Ecoverse agent installed
$ufaLast = @($ufaMaint | Where-Object { $_.Text -match 'Component status update\. Status: (\w+)' } | Select-Object -Last 1)
if ($registration -or $products) {
    $core3 = @($products | Where-Object { $_.Name -eq 'MSP Core Agent' })
    $exe = if ($registration) { $registration.Exe } else { $null }
    if ($core3.Count -and $exe -eq 'absent') {
        Add-Check 'Ecoverse agent installed' 'fail' 'registered, but its program (msp-agent-core.exe) is gone' @('The Windows Agent can then only try to remove it (which needs that program) and never installs a new one. The installer cannot fix this; the troubleshooting guide has the steps to remove the registration.')
    }
    elseif ($core3.Count) {
        $detail = @()
        if ($registration -and @($registration.Keys | Where-Object { $_ -eq 'absent' }).Count) { $detail += 'Some of its Windows Installer records are missing: ' + ($registration.Keys -join ', ') }
        Add-Check 'Ecoverse agent installed' $(if ($detail) { 'warn' } else { 'ok' }) ('MSP Core Agent {0}{1}' -f $core3[0].Version, $(if ($ufaLast.Count) { ', deployed by the Windows Agent' } else { '' })) $detail
    }
    else { Add-Check 'Ecoverse agent installed' 'warn' 'not installed (the Windows Agent deploys it a few minutes after itself, so give it time)' }
}
else { Add-Check 'Ecoverse agent installed' 'skip' 'could not read the installed products' }

# 3. Service
$svcState = if ($services -and $services.ContainsKey('MspAgent')) { $services['MspAgent'] } else { $null }
$hourAgo = $nowUtc.AddMinutes(-60)
$recentStarts = @($serviceStarts | Where-Object { $_ -ge $hourAgo }).Count
if ($services -and -not $svcState) { Add-Check 'MspAgent service' 'fail' 'there is no MspAgent service' @('The agent is not running at all. If the Ecoverse agent is registered the Windows Agent will not reinstall it; see the line above.') }
elseif ($svcState -and $svcState -ne 'Running') { Add-Check 'MspAgent service' 'fail' "$svcState" }
elseif ($recentStarts -ge 4) { Add-Check 'MspAgent service' 'warn' "running, but it started $recentStarts times in the last hour" @('A service that keeps restarting usually has a crash behind it: see the unexpected errors below and command.log.') }
elseif ($svcState) { Add-Check 'MspAgent service' 'ok' ('running; started {0}' -f $(if ($serviceStarts.Count) { Format-Clock $serviceStarts[$serviceStarts.Count - 1] } else { '?' })) }
elseif ($core.Count) { Add-Check 'MspAgent service' 'info' ('its log was last written {0}' -f (Format-Age $core[$core.Count - 1].At)) }
else { Add-Check 'MspAgent service' 'skip' 'could not read the services' }

# 4. Provisioned
if (-not $core.Count) { Add-Check 'Provisioned' 'fail' 'there is no core log to read' @("Looked for: $LogPath") }
elseif ($prov.ContainsKey('3')) {
    Add-Check 'Provisioned' 'ok' ('Success at {0}, {1} after the agent started' -f (Format-Clock $prov['3']), (Format-Duration ($prov['3'] - $install)))
}
elseif ($prov.ContainsKey('1')) {
    $wait = $nowUtc - $install
    Add-Check 'Provisioned' $(if ($wait.TotalMinutes -gt 30) { 'warn' } else { 'info' }) ('still setting up: InfoReceived since {0}' -f (Format-Clock $prov['1'])) @("It has been $(Format-Duration $wait) since the agent started. Setup normally takes about 5 minutes.")
}
else { Add-Check 'Provisioned' 'info' 'not provisioned yet' }

# 5. Login
$lastAuth = if ($oauth.Count) { $oauth[$oauth.Count - 1] } else { $null }
$loginDetail = @()
if ($oauth.Count) {
    $groups = @($oauth | Group-Object Result | ForEach-Object { '{0} x{1}' -f $(switch ($_.Name) { '200' { 'accepted' } 'ssl400' { '400 SSL certificate error' } 'timeout' { 'timeout' } 'reset' { 'connection reset' } default { "status $($_.Name)" } }), $_.Count })
    $loginDetail += 'Login attempts: ' + ($groups -join ', ')
    $loginDetail += ('First {0}, last {1}' -f (Format-Clock $oauth[0].At), (Format-Clock $lastAuth.At))
    $counts = @{ ok200 = 0; ssl400 = 0; timeout = 0; reset = 0 }
    foreach ($o in $oauth) { switch ($o.Result) { '200' { $counts.ok200++ } 'ssl400' { $counts.ssl400++ } 'timeout' { $counts.timeout++ } 'reset' { $counts.reset++ } } }
    $loginDetail += ('Counts: accepted {0}; 400 SSL certificate error {1}; timeouts {2}; connection resets {3}' -f $counts.ok200, $counts.ssl400, $counts.timeout, $counts.reset)
}
if ($sync.State -eq 'rejected') { Add-Check 'Login' 'fail' ('REJECTED: 400 "The SSL certificate error", last at {0}, never accepted since' -f (Format-Clock $sync.FaultLastAt)) $loginDetail }
elseif ($sync.State -eq 'blocked') { Add-Check 'Login' 'fail' ('BLOCKED: the login service does not answer, since {0}' -f (Format-Clock $sync.FaultSince)) $loginDetail }
elseif ($lastAuth -and $lastAuth.Result -eq '200') { Add-Check 'Login' 'ok' ('accepted at {0} ({1})' -f (Format-Clock $lastAuth.At), (Format-Age $lastAuth.At)) $loginDetail }
elseif ($lastAuth) { Add-Check 'Login' 'warn' ('last attempt at {0} was not accepted ({1})' -f (Format-Clock $lastAuth.At), $lastAuth.Result) $loginDetail }
elseif ($core.Count) { Add-Check 'Login' 'info' 'no login attempt in the log yet' }
else { Add-Check 'Login' 'skip' 'no core log' }

# 6. Device record
$recordGraceMinutes = 120
$sinceProvisioned = if ($prov.ContainsKey('3')) { $nowUtc - $prov['3'] } elseif ($install) { $nowUtc - $install } else { [timespan]::Zero }
$recordInGrace = $sinceProvisioned.TotalMinutes -lt $recordGraceMinutes
$recordDetail = @("Config replies without a device record: $($noConfig.Count) (no device configuration), $($noGoat.Count) (no GOAT NRI)")
if ($nri.Count) { $recordDetail += ('Components failing to get it: {0} times, first {1}, last {2}' -f $nri.Count, (Format-Clock $nri[0].At), (Format-Clock $nri[$nri.Count - 1].At)) }
if ($goatFound.Count) { $recordDetail += ('Config replies WITH a device record: {0}, the first at {1}' -f $goatFound.Count, (Format-Clock $goatFound[0])) }
if ($goatOther.Count) { $recordDetail += "Other lines that mention GOAT: $($goatOther.Count), the first: $($goatOther[0])" }
$recordDetail += 'The agent asks for its device record at provisioning and then at its hourly update check, so it can learn of a record up to an hour after it exists. Restarting the MspAgent service makes it ask at once.'
if ($sync.State -eq 'nodevice' -and $recordInGrace) {
    Add-Check 'Device record' 'warn' ('not received yet: the components have failed to get it since {0}; normal for the first {1} minutes after provisioning ({2} so far)' -f (Format-Clock $sync.FaultSince), $recordGraceMinutes, (Format-Duration $sinceProvisioned)) ($recordDetail + 'Seen on three healthy lab devices: the record arrived 86 to 94 minutes after provisioning. If it has not arrived by the end of that window it is a fault: run this check again then.')
}
elseif ($sync.State -eq 'nodevice') {
    Add-Check 'Device record' 'fail' ('the cloud sends this agent no device record (no GOAT NRI); since {0}, latest {1}; provisioned {2} ago' -f (Format-Clock $sync.FaultSince), (Format-Clock $sync.FaultLastAt), (Format-Duration $sinceProvisioned)) $recordDetail
}
elseif ($goatFound.Count) {
    $arrived = $goatFound[0]
    Add-Check 'Device record' 'ok' ('received at {0}{1}{2}' -f (Format-Clock $arrived), $(if ($prov.ContainsKey('3')) { ', {0} after provisioning' -f (Format-Duration ($arrived - $prov['3'])) } else { '' }), $(if ($nri.Count) { "; the components failed to get it $($nri.Count) times before that" } else { '' })) $recordDetail
}
elseif ($noGoat.Count -and -not $nri.Count) { Add-Check 'Device record' 'warn' 'the cloud said once that it has no device record for this agent; no component has failed on it yet' $recordDetail }
elseif ($core.Count) { Add-Check 'Device record' 'ok' ('no component is failing to get one{0}' -f $(if ($nri.Count) { " (it did earlier: $($nri.Count) errors, the last $(Format-Age $nri[$nri.Count - 1].At))" } else { '' })) $recordDetail }
else { Add-Check 'Device record' 'skip' 'no logs' }

# 7. Components
$byName = @($downloads | Group-Object Name | Sort-Object Name | ForEach-Object { $_.Name + $(if ($_.Count -gt 1) { " x$($_.Count)" } else { '' }) })
$downloadLast = if ($downloads.Count) { $downloads[$downloads.Count - 1].At } else { $null }
$stillDenied = $denied -ge 5 -and (-not $downloadLast -or $deniedLast -gt $downloadLast)
if ($stillDenied) {
    $detail = @("Refused with 403: $denied times, the last at $(Format-Clock $deniedLast); the last successful download was $(if ($downloadLast) { Format-Clock $downloadLast } else { 'never' })")
    if ($byName) { $detail += 'Downloaded before: ' + ($byName -join ', ') }
    $detail += 'A restart does not help; the installer does a one-time repair after 30 minutes. If the CDN refuses even after that, report it to N-able.'
    Add-Check 'Components' 'fail' ('downloads are being REFUSED (403) by the component CDN: {0} refusals, none accepted since' -f $denied) $detail
}
elseif ($downloads.Count) {
    $detail = @('Downloaded: ' + ($byName -join ', '))
    if ($denied) { $detail += "CDN refusals (403): $denied, the last at $(Format-Clock $deniedLast); downloads worked after that" }
    if ($reacquired) { $detail += "Re-fetched after a failed integrity check: $reacquired (normal right after an install)" }
    if ($upgrades) { $detail += "The core upgraded itself $upgrades time(s)" }
    Add-Check 'Components' $(if ($denied -ge 5) { 'warn' } else { 'ok' }) ('{0} file(s) downloaded, {1} different components{2}' -f $downloads.Count, @($downloads | Select-Object -ExpandProperty Name -Unique).Count, $(if ($denied) { "; $denied refused (403) earlier" } else { '' })) $detail
}
elseif ($core.Count) { Add-Check 'Components' 'info' 'none downloaded yet' }
else { Add-Check 'Components' 'skip' 'no logs' }

# 8. Inventory collected
function Get-ComponentLog([string]$Name) {
    if (-not (Test-Path -LiteralPath $src.Components)) { return @() }
    @(Get-ChildItem -LiteralPath $src.Components -Directory -Filter $Name -ErrorAction SilentlyContinue | ForEach-Object { Get-ChildItem -LiteralPath $_.FullName -Recurse -File -Filter "$Name.log" -ErrorAction SilentlyContinue })
}
$interrogator = @(Get-ComponentLog 'generic-asset-interrogator')
$interRows = if ($interrogator.Count) { @(Read-StampedLog $interrogator[0].FullName) } else { @() }
$collectorLogs = if ($interrogator.Count) { @(Get-ChildItem -LiteralPath (Split-Path $interrogator[0].FullName -Parent) -File -Filter '*-hardware-*.log' -ErrorAction SilentlyContinue).Count } else { 0 }
if ($interrogator.Count -and $interRows.Count) {
    Add-Check 'Inventory collected' 'ok' ('the inventory component last ran {0} ({1}); {2} collector(s) have run' -f (Format-Clock $interRows[$interRows.Count - 1].At), (Format-Age $interRows[$interRows.Count - 1].At), $collectorLogs)
}
elseif ($interrogator.Count) { Add-Check 'Inventory collected' 'info' 'the inventory component is installed but has not logged yet' }
elseif ($downloads | Where-Object { $_.Name -eq 'generic-asset-interrogator' }) { Add-Check 'Inventory collected' 'warn' 'the inventory component was downloaded but has no log of its own' }
else { Add-Check 'Inventory collected' 'info' 'the inventory component is not on the device yet' }

# 9. Data leaves the device
$sendDetail = @()
if ($flushes.Count) { $sendDetail += ('Components asked to send their data {0} times, the last {1}' -f $flushes.Count, (Format-Age $flushes[$flushes.Count - 1].At)) }
if ($sync.State -eq 'nodevice' -and $recordInGrace) { Add-Check 'Data sent' 'info' 'waiting for the device record: until it arrives every flush stops at "failed to get goat NRI" without sending' $sendDetail }
elseif ($sync.State -eq 'nodevice') { Add-Check 'Data sent' 'fail' 'cannot send anything: every flush stops at "failed to get goat NRI" and closes the connection without sending' ($sendDetail + 'A restart helps only if the device record already exists in the cloud (it makes the agent ask now instead of hourly). The one-time repair changed nothing on the devices that waited 4 hours (sedemo). If a restart does not bring the record, it is fixed in the Ecoverse tenant.') }
elseif ($sync.State -in 'rejected', 'blocked', 'denied') { Add-Check 'Data sent' 'skip' 'cannot send until the fault above is fixed' }
elseif ($inventorySent.Count) {
    $detail = $sendDetail + @('Sent by type: ' + (@($sent | Group-Object Type | Sort-Object Count -Descending | Select-Object -First 8 | ForEach-Object { '{0} x{1}' -f $_.Name, $_.Count }) -join ', '))
    if ($sentFailed.Count) { $detail += @($sentFailed | Group-Object Type, Why | ForEach-Object { 'A batch was refused: {0} x{1} ({2}); the agent re-queues it' -f $_.Group[0].Type, $_.Count, $_.Group[0].Why }) }
    Add-Check 'Data sent' 'ok' ('the inventory (DeviceAsset) left the device {0} time(s), the last {1} ({2}){3}' -f $inventorySent.Count, (Format-Clock $inventorySent[$inventorySent.Count - 1].At), (Format-Age $inventorySent[$inventorySent.Count - 1].At), $(if ($sentFailed.Count) { "; $($sentFailed.Count) batch(es) were refused and re-queued" } else { '' })) $detail
}
elseif ($sent.Count) { Add-Check 'Data sent' 'info' ('{0} batch(es) were sent, the last {1}, but none of them is the inventory (DeviceAsset) yet' -f $sent.Count, (Format-Age $sent[$sent.Count - 1].At)) $sendDetail }
elseif ($core.Count) { Add-Check 'Data sent' 'info' 'no sending error found, and no batch was logged as sent either. This script cannot see Ecoverse: confirm the data arrived in N-voke (Track: Ecoverse data received)' $sendDetail }
else { Add-Check 'Data sent' 'skip' 'no logs' }

# 10. Script channel (what N-voke's Probe and Status use)
$exec = @(Get-ComponentLog 'msp-ufa-script-execution')
if ($exec.Count) {
    $execRows = @(Read-StampedLog $exec[0].FullName)
    $runs = @($execRows | Where-Object { $_.Text -match 'execution::logOutputParam Action \[Action\]: (\w+)' } | ForEach-Object { [pscustomobject]@{ At = $_.At; Action = $Matches[1] } })
    $decrypt = @($execRows | Where-Object { $_.Text -match 'failed to decrypt value' })
    $detail = @()
    if ($runs.Count) { $detail += 'Runs: ' + (@($runs | Group-Object Action | ForEach-Object { '{0} x{1}' -f $_.Name, $_.Count }) -join ', ') }
    if ($decrypt.Count) { $detail += "$($decrypt.Count) 'failed to decrypt value' error(s): a script input that is encrypted for this device could not be read (last $(Format-Age $decrypt[$decrypt.Count - 1].At))" }
    if ($runs.Count) { Add-Check 'Script channel' 'ok' ('scripts reach this device: {0} run(s), the last {1} ({2})' -f $runs.Count, (Format-Clock $runs[$runs.Count - 1].At), (Format-Age $runs[$runs.Count - 1].At)) $detail }
    else { Add-Check 'Script channel' 'info' 'the script component is installed; no script has run through it' $detail }
}
else { Add-Check 'Script channel' 'info' 'the script component is not on the device yet' }

# 11. Windows Agent maintenance of the Ecoverse agent
$waUninstalls = @($ufaMaint | Where-Object { $_.Text -match 'Component status update\. Status: Uninstalling' } | ForEach-Object {
        $at = $_.At
        [pscustomobject]@{ At = $at; MissingService = [bool](@($ufaMaint | Where-Object { $_.At -le $at -and $_.At -ge $at.AddSeconds(-5) -and $_.Text -match 'service \[ MspAgent \] not exists' }).Count) }
    })
$nvokeCleanups = New-Object Collections.Generic.List[datetime]
foreach ($f in @(Get-ChildItem -LiteralPath (Join-Path $src.Nvoke 'logs') -File -Filter 'install-*.log' -ErrorAction SilentlyContinue)) {
    foreach ($line in (Read-LogLine $f.FullName)) {
        if ($line -match '^(\d{4}-\d\d-\d\dT[\d:.]+[+-]\d\d:\d\d)\s+cleanup-(ufa-uninstall|uninstall|ufa-wait)') { $nvokeCleanups.Add([datetimeoffset]::Parse($Matches[1], [Globalization.CultureInfo]::InvariantCulture).UtcDateTime) }
    }
}
# Who removed the MSP Core Agent: the Windows Agent's own hourly maintenance, the N-voke cleanup of a move, or neither.
function Get-RemovalCause([datetime]$At) {
    $wa = @($waUninstalls | Where-Object { [math]::Abs(($_.At - $At).TotalSeconds) -lt 90 } | Select-Object -First 1)
    if ($wa.Count) { return $(if ($wa[0].MissingService) { 'by the Windows Agent (its hourly check found no MspAgent service)' } else { 'by the Windows Agent' }) }
    if (@($nvokeCleanups | Where-Object { [math]::Abs(($_ - $At).TotalSeconds) -lt 180 }).Count) { return 'by the N-voke cleanup (the move)' }
    if ($ufaMaint.Count -and $At -lt $ufaMaint[0].At) { return 'cause is not in the logs (the Windows Agent log starts later: a move replaces the Windows Agent and its log)' }
    'cause is not in the logs'
}
if ($ufaMaint.Count) {
    $updates = @($ufaMaint | Where-Object { $_.Text -match 'Component status update\. Status: (\w+)(?:, Error Code: (-?\d+))?' } | ForEach-Object { [pscustomobject]@{ At = $_.At; Status = $Matches[1]; Code = $Matches[2] } })
    $suspended = @($ufaMaint | Where-Object { $_.Text -match 'Install configurations are not present' })
    $bad = @($updates | Where-Object { $_.Code -and $_.Code -ne '0' })
    $detail = @()
    if ($updates.Count) { $detail += 'Recent: ' + (@($updates | Select-Object -Last 6 | ForEach-Object { '{0} {1}{2}' -f (Format-Clock $_.At), $_.Status, $(if ($_.Code -and $_.Code -ne '0') { " (error $($_.Code))" } else { '' }) }) -join '; ') }
    # Every deployment starts with this line until the server's configuration arrives seconds later: only a run that ends there is a problem.
    $lastSuspended = if ($suspended.Count) { $suspended[$suspended.Count - 1].At } else { $null }
    $progressed = $lastSuspended -and @($ufaMaint | Where-Object { $_.At -gt $lastSuspended -and $_.Text -match 'Starting to install|Detected installed agent|Install command line' }).Count
    $stuckSuspended = $lastSuspended -and $lastSuspended -ge $hourAgo -and -not $progressed
    if ($bad.Count) { Add-Check 'Windows Agent deploys it' 'fail' ('{0} install or removal attempt(s) ended in an error code; the last at {1}' -f $bad.Count, (Format-Clock $bad[$bad.Count - 1].At)) $detail }
    elseif ($stuckSuspended) { Add-Check 'Windows Agent deploys it' 'warn' 'the Windows Agent has no install configuration for the Ecoverse agent ("Install configurations are not present") and has not received one since' ($detail + 'It cannot deploy the agent until the server sends it one: check that the destination N-central has the Ecoverse agent enabled for this customer.') }
    elseif ($updates.Count) { Add-Check 'Windows Agent deploys it' 'ok' ('{0} install/removal step(s), all with error code 0; the last {1} {2}' -f $updates.Count, $updates[$updates.Count - 1].Status, (Format-Age $updates[$updates.Count - 1].At)) $detail }
    else { Add-Check 'Windows Agent deploys it' 'info' 'the Windows Agent log has nothing about deploying the agent' }
}
else { Add-Check 'Windows Agent deploys it' 'skip' 'no UfaMaint.log (the Windows Agent log) to read' }

# 12. Windows Installer
if ($msiEvents) {
    $failed = @($msiEvents | Where-Object { $_.Id -in 11708, 11725 })
    $okEvents = @($msiEvents | Where-Object { $_.Id -in 11707, 11724 })
    $lastFail = if ($failed.Count) { $failed[$failed.Count - 1] } else { $null }
    $lastOk = if ($okEvents.Count) { $okEvents[$okEvents.Count - 1] } else { $null }
    if ($lastFail -and (-not $lastOk -or $lastFail.At -gt $lastOk.At)) {
        $why = @($msiEvents | Where-Object { $_.Id -in 11721, 11722 -and [math]::Abs(($_.At - $lastFail.At).TotalSeconds) -lt 5 } | Select-Object -Last 1 | ForEach-Object { $_.Message })
        Add-Check 'Windows Installer' 'fail' ('the last install/removal of MSP Core Agent FAILED at {0} ({1} failed in 7 days)' -f (Format-Clock $lastFail.At), $failed.Count) @($(if ($why) { $why[0] } else { $lastFail.Message }))
    }
    elseif ($failed.Count) { Add-Check 'Windows Installer' 'ok' ('the last install/removal succeeded ({0}); {1} failed earlier this week' -f (Format-Age $lastOk.At), $failed.Count) }
    else { Add-Check 'Windows Installer' 'ok' ('no failed install or removal in 7 days ({0} event(s))' -f $msiEvents.Count) }
}
else { Add-Check 'Windows Installer' 'skip' 'could not read the Windows Installer events' }

# 12b. How often the Ecoverse agent was taken off and put back, and by whom. Every redeploy provisions a brand-new agent.
if ($msiEvents) {
    $removals = @($msiEvents | Where-Object { $_.Id -eq 11724 -and $_.Message -match 'MSP Core Agent' -and $_.At -ge $nowUtc.AddHours(-24) })
    $causes = @($removals | ForEach-Object { [pscustomobject]@{ At = $_.At; Cause = (Get-RemovalCause $_.At) } })
    $byAgent = @($causes | Where-Object { $_.Cause -like 'by the Windows Agent*' })
    $detail = @($causes | ForEach-Object { '{0} removed {1}' -f (Format-Clock $_.At), $_.Cause })
    if ($byAgent.Count -ge 2) {
        Add-Check 'Redeploys' 'warn' ('the Windows Agent took the Ecoverse agent off and put it back {0} times in 24 hours' -f $byAgent.Count) ($detail + 'Its hourly check removes the agent when it finds no MspAgent service, and the redeploy provisions a brand-new agent, which starts the wait for its device record over (20 to 90 minutes). Look at what removes the service before the check runs.')
    }
    elseif ($causes.Count) { Add-Check 'Redeploys' 'ok' ('{0} removal(s) in 24 hours, {1}' -f $causes.Count, $(if (@($causes | Where-Object { $_.Cause -like 'by the N-voke*' }).Count -eq $causes.Count) { 'all by the N-voke cleanup of a move' } else { 'see the list' })) $detail }
    else { Add-Check 'Redeploys' 'ok' 'the Ecoverse agent was not removed or reinstalled in the last 24 hours' }
}

# 13. N-voke installer (its own record, against what the log says now)
if ($installer) {
    $said = if ($installer.ufa -and $installer.ufa.sync) { [string]$installer.ufa.sync } else { $null }
    $detail = @(('Stage {0}, {1} attempt(s){2}' -f $installer.stage, $installer.attempts, $(if ($installer.healthyAt) { ", healthy at $(Format-Clock ([datetime]::Parse($installer.healthyAt, [Globalization.CultureInfo]::InvariantCulture, $utcStyle)))" } else { '' })))
    if ($installer.warning) { $detail += "Warning it recorded: $($installer.warning)" }
    if ($installer.ufaRepairedAt) { $detail += "The one-time repair was done at $(Format-Clock ([datetime]::Parse($installer.ufaRepairedAt, [Globalization.CultureInfo]::InvariantCulture, $utcStyle)))" }
    if ($installer.ufaSyncRestarts) { $detail += "MspAgent restarted by the watcher: $($installer.ufaSyncRestarts)" }
    if ($said -and $said -ne $sync.State -and $sync.State -ne 'unknown') {
        Add-Check 'N-voke installer' 'warn' ("its saved verdict is out of date: it recorded '$said', the logs say '$($sync.State)' now") ($detail + 'The installer decides once and stops watching; trust this report over its saved word.')
    }
    elseif ($installer.stage -in 'failed', 'error') { Add-Check 'N-voke installer' 'fail' "stage '$($installer.stage)'" $detail }
    elseif ($installer.warning) { Add-Check 'N-voke installer' 'warn' "stage '$($installer.stage)' with a warning" $detail }
    else { Add-Check 'N-voke installer' 'ok' ("stage '{0}'{1}" -f $installer.stage, $(if ($said) { ", it recorded '$said'" } else { '' })) $detail }
}
else { Add-Check 'N-voke installer' 'skip' 'no status.json (not a device moved by N-voke, or already cleaned up)' }

# 14. Clock and network
if ($clock) { Add-Check 'Clock' $(if ($clock.Synced) { 'ok' } else { 'info' }) ('{0}{1}' -f $clock.Source, $(if ($clock.Synced) { '' } else { ' (not synchronized with a time server; the time was right when checked, but it can drift)' })) }
else { Add-Check 'Clock' 'skip' $(if ($script:ClockNote) { "could not read the time source ($script:ClockNote)" } else { 'could not read the time source' }) }
if ($net) {
    $bad = @($net | Where-Object { -not $_.Ok } | Select-Object -ExpandProperty Host -Unique)
    $detail = @($net | Group-Object Host | ForEach-Object { '{0}: {1} of {2} connected, {3}-{4} ms' -f $_.Name, @($_.Group | Where-Object Ok).Count, $_.Count, ($_.Group | Measure-Object Ms -Minimum).Minimum, ($_.Group | Measure-Object Ms -Maximum).Maximum })
    if ($bad.Count) { Add-Check 'Network (TCP 443)' 'fail' ('cannot connect to: ' + ($bad -join ', ')) $detail }
    else { Add-Check 'Network (TCP 443)' 'ok' ('all {0} hosts connected' -f @($net | Select-Object -ExpandProperty Host -Unique).Count) $detail }
}
else { Add-Check 'Network (TCP 443)' 'skip' 'not tested (add -Network)' }

# ---- the errors nobody expected -----------------------------------------------------------------------------------
# Known noise: seen on healthy devices or explained elsewhere in this report. Anything else is listed.
$benign = @(
    'localdiscovery::.*(169\.254\.169\.254|cloud discovery|FingerPrint)', 'hashstorage::getConfigStringValue failed to read', 'StartTakeControlDetection .* not implemented',
    'cove::|adlumin::|registry::ReadValue failed to open registry key', 'database::(getListSize|getConfigQoS|handleError)', 'cache::getValue failed to read', 'protobuilder::readScheduleState',
    'collectFingerPrintData failed to discover local data',
    'failed integrity check', 'refreshDeviceInfo failed to get goat NRI', 'messaging::flushQueues failed to create ingest client', 'messaging::initialiseIngestClient failed to refresh JWT'
)
# Before the agent is provisioned it has no certificate, so these are the expected result of that.
$setupNoise = 'certmanagement::|auth::|checkOauthToken|collectCertThumbprint|getFingerprintData|configservice::|fetchconfig::|main::fetchDeviceConfiguration|messaging::Connect|main::checkForUpdates'
function Get-ErrorShape([string]$Text) {
    $shape = $Text -replace '[0-9a-fA-F]{8}-[0-9a-fA-F-]{27}', '<id>' -replace '[A-Za-z0-9+/]{40,}={0,2}', '<b64>' -replace '\b\d+\b', 'N'
    if ($shape.Length -gt 150) { $shape.Substring(0, 150) + '...' } else { $shape }
}
$provisionedAt = if ($prov.ContainsKey('3')) { $prov['3'] } else { $null }
# What the fault already reported above produces as a side effect, so it is not listed again as a surprise.
$explainedBy = switch ($sync.State) { 'rejected' { 'SSL certificate error' } 'blocked' { 'Client\.Timeout|unable to get jwt|forcibly closed|connection reset' } 'denied' { '''403''' } default { $null } }
$explained = 0
$unexpected = @{}
$setupCount = 0
$sources = New-Object Collections.Generic.List[object]
$sources.Add(@('core', $core)); $sources.Add(@('component.log', $comp))
foreach ($name in 'command', 'provision', 'deprovision', 'database') { $sources.Add(@("$name.log", @(Read-StampedLog (Join-Path $src.Logs "$name.log")))) }
if (Test-Path -LiteralPath $src.Components) {
    foreach ($f in @(Get-ChildItem -LiteralPath $src.Components -Recurse -File -Filter '*.log' -ErrorAction SilentlyContinue | Where-Object { $_.Name -notmatch '-hardware-' })) { $sources.Add(@($f.Name, @(Read-StampedLog $f.FullName))) }
}
foreach ($source in $sources) {
    foreach ($r in $source[1]) {
        if ($r.Level -ne 'ERROR') { continue }
        if ($benign | Where-Object { $r.Text -match $_ }) { continue }
        if ($r.Text -match $setupNoise -and (-not $provisionedAt -or $r.At -lt $provisionedAt.AddMinutes(3))) { $setupCount++; continue }
        if ($explainedBy -and $r.Text -match $explainedBy) { $explained++; continue }
        $key = $source[0] + '|' + (Get-ErrorShape $r.Text)
        if (-not $unexpected.ContainsKey($key)) { $unexpected[$key] = [pscustomobject]@{ Where = $source[0]; Shape = (Get-ErrorShape $r.Text); Count = 0; First = $r.At; Last = $r.At } }
        $unexpected[$key].Count++; $unexpected[$key].Last = $r.At
    }
}
$unexpectedList = @($unexpected.Values | Sort-Object Last -Descending)
Add-Check 'Unexpected errors' $(if ($unexpectedList.Count) { 'warn' } else { 'ok' }) $(if ($unexpectedList.Count) { '{0} kind(s) of error that nothing above explains (listed below)' -f $unexpectedList.Count } else { 'none beyond the known noise' }) @()

# ---- the verdict ---------------------------------------------------------------------------------------------------
$advice = @{
    nodevice = 'The agent asks for its device record at provisioning and then hourly, so a record that exists can go unnoticed for up to an hour: restarting the MspAgent service makes it ask now. If a restart does not bring the record, or it is still missing after about 2 hours (healthy devices got it after 86 to 94 minutes), it has to be fixed in the destination''s Ecoverse tenant: the customer or site this device registered under must exist there and be linked to N-central, and other devices there should show data. Then report it to N-able with the logs (collect-logs.ps1). The one-time repair changed nothing on the devices that waited 4 hours.'
    rejected = 'A restart does not help. The installer repairs once after 30 minutes. If it is still refused after that, check TLS inspection, proxy and firewall on this device''s network path, and report it to N-able.'
    blocked  = 'The login service does not answer this device. It often clears by itself; if it lasts, the installer restarts MspAgent after 30 minutes and repairs once after 60. Check that the device can reach the auth host on port 443 with no TLS inspection (add -Network).'
    denied   = 'The component CDN refuses the downloads (403). A restart does not help; the installer repairs once after 30 minutes.'
    stalled  = 'The first configuration fetch failed and the agent retries only hourly. The installer restarts MspAgent for this, at most twice.'
    waiting  = 'Nothing is wrong yet. Provisioning normally takes about 5 minutes and the first data within an hour.'
}
$fails = @($checks | Where-Object Status -eq 'fail')
$warns = @($checks | Where-Object Status -eq 'warn')
if ($fails.Count) {
    $headline = 'PROBLEM - ' + $fails[0].Name + ': ' + $fails[0].Summary
    $otherFails = @($fails | Select-Object -Skip 1 | Where-Object { $_.Name -ne 'Data sent' })
    if ($otherFails.Count) { $headline += ' ALSO FAILING: ' + (($otherFails | ForEach-Object { $_.Name }) -join ', ') + '.' }
    $next = if ($advice.ContainsKey($sync.State) -and $sync.State -notin 'waiting', 'stalled') { $advice[$sync.State] } elseif ($fails[0].Detail.Count) { $fails[0].Detail[$fails[0].Detail.Count - 1] } else { 'See the evidence below.' }
    if ($otherFails | Where-Object { $_.Name -eq 'Components' }) { $next += ' ALSO, for the components: ' + $advice['denied'] }
}
elseif ($sync.State -eq 'waiting' -or ($sync.State -eq 'stalled')) { $headline = 'IN PROGRESS - the agent is set up and no fault was found yet, but its inventory component has not arrived'; $next = $advice[$sync.State] }
elseif ($sync.State -eq 'nodevice' -and $recordInGrace) {
    $deadline = if ($prov.ContainsKey('3')) { $prov['3'] } else { $install }
    $headline = 'IN PROGRESS - the agent is set up and logging in, and waiting for its device record (normal for the first {0} minutes after provisioning; {1} so far)' -f $recordGraceMinutes, (Format-Duration $sinceProvisioned)
    $others = @($warns | Where-Object { $_.Name -notin 'Device record', 'N-voke installer' })
    if ($others.Count) { $headline += ' Also look at: ' + (($others | ForEach-Object { $_.Name }) -join ', ') + '.' }
    $next = 'Run this check again in 30 minutes. If the device record has still not arrived by {0}, it is a fault. {1}' -f (Format-Clock $deadline.AddMinutes($recordGraceMinutes)), $advice['nodevice']
}
elseif ($warns.Count) { $headline = 'NO FAULT FOUND, with {0} thing(s) to look at: {1}' -f $warns.Count, (($warns | ForEach-Object { $_.Name }) -join ', '); $next = 'The data cannot be confirmed from the device: check Track in N-voke.' }
else { $headline = 'NO FAULT FOUND ON THE DEVICE'; $next = 'The data cannot be confirmed from the device: check Track in N-voke (Ecoverse data received).' }

# ---- the report ----------------------------------------------------------------------------------------------------
$out = New-Object Collections.Generic.List[string]
$width = 104
function Add-Wrapped([string]$Text, [int]$Indent = 0, [string]$First = '') {
    # Wrapped lines line up under where the text starts.
    $pad = ' ' * $(if ($First) { $First.Length } else { $Indent })
    $line = if ($First) { $First } else { $pad }
    $lead = $line
    foreach ($word in ($Text -split '\s+')) {
        if ($line.Length + $word.Length + 1 -gt $width -and $line -ne $lead) { $out.Add($line.TrimEnd()); $line = $pad + $word }
        elseif ($line -eq $lead) { $line = $line + $word }
        else { $line = $line + ' ' + $word }
    }
    if ($line.Trim().Length) { $out.Add($line.TrimEnd()) }
}
$computer = if ($env:COMPUTERNAME) { $env:COMPUTERNAME } else { [Environment]::MachineName }
$deviceName = if ($systemText -match 'Computer: (\S+)') { $Matches[1] } else { $computer }
$bar = '=' * $width
$out.Add($bar)
$out.Add(('  Ecoverse agent check - {0} - {1}{2}' -f $deviceName, ($nowUtc + $script:Offset).ToString('yyyy-MM-dd HH:mm'), $(if ($root) { '  (from collected logs)' } else { '' })))
$out.Add($bar)
Add-Wrapped $headline 12 '  RESULT   '
Add-Wrapped $next 12 '  NEXT     '
$out.Add(('  STATE: {0}' -f $sync.State) + $(switch ($sync.State) {
    'synced' { ' (data is flowing: the device record arrived or the inventory was sent)' }
    'nodevice' { ' (logs in and its components run, but the cloud has no device record for it, so none of its data can be sent)' }
    'rejected' { ' (the cloud refuses the agent''s login)' }
    'denied' { ' (the component CDN answers the agent with 403)' }
    'blocked' { ' (the cloud does not answer the agent''s login)' }
    'stalled' { ' (login works again, nothing delivered)' }
    'waiting' { ' (no fault, and no proof yet that data is leaving the device)' }
    default { ' (no log to read)' } }))
if ($sync.FaultSince) { $out.Add(('         failing since {0}, last failure {1} (device time)' -f (Format-Clock $sync.FaultSince), $(if ($sync.FaultLastAt) { Format-Clock $sync.FaultLastAt } else { '-' }))) }
$out.Add('')
$out.Add('  STAGES, in the order a device gets there')
$out.Add('  ' + ('-' * ($width - 2)))
foreach ($c in $checks) {
    if ($c.Name -eq 'Unexpected errors') { $out.Add(''); $out.Add('  AND') }
    Add-Wrapped $c.Summary 33 ('  {0} {1,-24} ' -f $mark[$c.Status], $c.Name)
}

$evidence = @($checks | Where-Object { $_.Detail.Count -and ($Full -or $_.Status -in 'warn', 'fail') })
if ($evidence.Count) {
    $out.Add(''); $out.Add('  EVIDENCE'); $out.Add('  ' + ('-' * ($width - 2)))
    foreach ($c in $evidence) {
        $out.Add(('  {0}' -f $c.Name))
        foreach ($d in $c.Detail) { Add-Wrapped $d 6 '    - ' }
    }
}

$out.Add(''); $out.Add('  N-VOKE INSTALLER, in its own words'); $out.Add('  ' + ('-' * ($width - 2)))
if ($installer) {
    $history = @($installer.history | Select-Object -Last $(if ($Full) { 40 } else { 8 }))
    foreach ($h in $history) { Add-Wrapped ([string]$h.detail) 0 ('    {0}  {1,-24} ' -f (Format-Clock ([datetime]::Parse($h.at, [Globalization.CultureInfo]::InvariantCulture, $utcStyle))), $h.event) }
    $latest = @(Get-ChildItem -LiteralPath (Join-Path $src.Nvoke 'logs') -File -Filter 'install-*.log' -ErrorAction SilentlyContinue | Sort-Object Name)
    $cleanup = @(); foreach ($f in $latest) { $cleanup += @(Read-LogLine $f.FullName | Where-Object { $_ -match 'cleanup-ufa|Msp Agent|ufa-repair' }) }
    if ($cleanup.Count) {
        $out.Add('    What the cleanup did about the Ecoverse agent (install log):')
        foreach ($c in ($cleanup | Select-Object -First 6)) { Add-Wrapped (($c -replace '^\S+\s', '')) 8 '      * ' }
    }
}
else { $out.Add('    (no status.json)') }

# The timeline: what happened, merged from every log, newest last.
$timeline = New-Object Collections.Generic.List[object]
function Add-TimelineItem($At, [string]$Source, [string]$Text) { if ($At) { $timeline.Add([pscustomobject]@{ At = [datetime]$At; Source = $Source; Text = $Text }) } }
if ($install) { Add-TimelineItem $install 'agent' 'the Ecoverse agent started for the first time' }
foreach ($k in '1', '3') { if ($prov.ContainsKey($k)) { Add-TimelineItem $prov[$k] 'agent' $(if ($k -eq '3') { 'provisioned (state 3, Success)' } else { 'provisioning: information received' }) } }
$lastResult = $null
foreach ($o in $oauth) { if ($o.Result -ne $lastResult) { Add-TimelineItem $o.At 'login' $(switch ($o.Result) { '200' { 'login accepted' } 'ssl400' { 'login REJECTED (400 SSL certificate error)' } 'timeout' { 'login timed out' } 'reset' { 'login connection reset' } default { "login status $($o.Result)" } }); $lastResult = $o.Result } }
foreach ($g in @($downloads | Group-Object { $_.At.ToString('yyyy-MM-dd HH:mm') })) { Add-TimelineItem $g.Group[0].At 'components' ('downloaded {0} component file(s): {1}' -f $g.Count, ((@($g.Group | Select-Object -ExpandProperty Name -Unique) | Select-Object -First 4) -join ', ')) }
if ($noGoat.Count) { Add-TimelineItem $noGoat[0] 'device record' 'the cloud sent no device record (no GOAT NRI)' }
if ($goatFound.Count) { Add-TimelineItem $goatFound[0] 'device record' 'the cloud sent the device record (GOAT NRI found in the config reply)' }
if ($inventorySent.Count) { Add-TimelineItem $inventorySent[0].At 'data sent' 'the inventory (DeviceAsset) was sent for the first time' }
if ($nri.Count) { Add-TimelineItem $nri[0].At 'device record' 'a component first failed to get the device record'; if ($nri.Count -gt 1) { Add-TimelineItem $nri[$nri.Count - 1].At 'device record' "the same failure again (latest of $($nri.Count))" } }
if ($interRows.Count) { Add-TimelineItem $interRows[0].At 'inventory' 'the inventory component first ran' }
foreach ($u in $ufaMaint) { if ($u.Text -match 'Component status update\. Status: (\w+)(?:, Error Code: (-?\d+))?') { Add-TimelineItem $u.At 'windows agent' ('Ecoverse agent {0}{1}' -f $Matches[1].ToLower(), $(if ($Matches[2] -and $Matches[2] -ne '0') { " (error $($Matches[2]))" } else { '' })) } }
foreach ($e in @($msiEvents)) { if ($e.Id -in 11707, 11724, 11725, 11721, 11722 -and ($Full -or $e.At -ge $nowUtc.AddHours(-12))) { Add-TimelineItem $e.At 'installer' $(switch ($e.Id) { 11707 { 'MSP Core Agent installed' } 11724 { 'MSP Core Agent removed ' + (Get-RemovalCause $e.At) } 11725 { 'MSP Core Agent removal FAILED' } 11721 { 'MSP Core Agent: error 1721 (its program could not be run)' } 11722 { 'MSP Core Agent: error 1722 (a step did not finish)' } }) } }
if ($installer) { foreach ($h in @($installer.history)) { if ($h.event -in 'ufa-repair', 'ufa-sync-restart', 'ufa-repair-restart', 'healthy', 'ufa-registration-broken') { Add-TimelineItem ([datetime]::Parse($h.at, [Globalization.CultureInfo]::InvariantCulture, $utcStyle)) 'n-voke' ('{0}: {1}' -f $h.event, $h.detail) } } }
$items = @($timeline | Sort-Object At | Group-Object { '{0:yyyyMMddHHmm}|{1}|{2}' -f $_.At, $_.Source, $_.Text } | ForEach-Object { $_.Group[0] } | Sort-Object At)
$limit = if ($Full) { 200 } else { 28 }
if ($items.Count) {
    $out.Add(''); $out.Add(('  TIMELINE (device time{0})' -f $(if ($items.Count -gt $limit) { ", the last $limit of $($items.Count); -Full for all" } else { '' }))); $out.Add('  ' + ('-' * ($width - 2)))
    foreach ($i in ($items | Select-Object -Last $limit)) { Add-Wrapped $i.Text 31 ('    {0,-11} {1,-14} ' -f (Format-Clock $i.At), $i.Source) }
}

if ($unexpectedList.Count) {
    $out.Add(''); $out.Add('  UNEXPECTED ERRORS (not known noise, not explained above)'); $out.Add('  ' + ('-' * ($width - 2)))
    foreach ($e in ($unexpectedList | Select-Object -First $(if ($Full) { 60 } else { 12 }))) {
        Add-Wrapped ('[{0}] {1}' -f $e.Where, $e.Shape) 0 ('    x{0,-4} {1}-{2}  ' -f $e.Count, (Format-Clock $e.First), (Format-Clock $e.Last))
    }
    if ($unexpectedList.Count -gt 12 -and -not $Full) { $out.Add("    ... and $($unexpectedList.Count - 12) more (-Full)") }
}
if ($setupCount) { $out.Add(''); $out.Add(('  ({0} error line(s) from before provisioning were left out: the agent has no certificate until it is provisioned.)' -f $setupCount)) }
if ($explained) { $out.Add(('  ({0} error line(s) were left out: they are the same fault as the {1} state above.)' -f $explained, $sync.State)) }
$out.Add('')
$out.Add(('  Read: {0}' -f $(if ($Collected) { $Collected } else { $LogPath })))
$out.Add('  Cannot see Ecoverse itself: its side is the Track step in N-voke.')

$text = $out -join "`r`n"
Write-Output $text
if ($OutFile) { Set-Content -LiteralPath $OutFile -Value $text -Encoding UTF8 }
if ($cleanupRoot) { Remove-Item -LiteralPath $cleanupRoot -Recurse -Force -ErrorAction SilentlyContinue }
