<#
.SYNOPSIS
    Summarises the Ecoverse agent's (UFA) core log: provisioning, login-token results, the hourly update
    checks, inventory components and service restarts. Read-only. Prints no secrets.
.DESCRIPTION
    Run on the device, in Windows PowerShell 5.1 or PowerShell 7:
        powershell -NoProfile -ExecutionPolicy Bypass -File ufa-log-summary.ps1
    or on a copied log:
        powershell -NoProfile -File ufa-log-summary.ps1 -LogPath C:\temp\msp-agent-core.log
    Add -OutFile C:\temp\summary.txt to save the output as well.
    Times are shown as written in the log (the device's local time).
#>
param(
    [string]$LogPath = ([Environment]::GetFolderPath('ProgramFilesX86') + '\Msp Agent\logs\msp-agent-core.log'),
    [string]$OutFile
)
$ErrorActionPreference = 'Stop'
if (-not (Test-Path -LiteralPath $LogPath)) { Write-Output "No log at $LogPath"; exit 1 }

# BEGIN shared with endpoint/installAgent.ps1. This is a verbatim copy of its Get-UfaSyncState, so the STATE below is the
# one the installer's watcher acts on. Endpoint.Tests.ps1 fails if the two copies differ: change it there, then copy it here.
function Get-UfaSyncState([string]$LogPath, [datetime]$Since) {
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
    $state = if ($syncedAt -and (-not $failedAt -or $syncedAt -gt $failedAt)) { 'synced' }
             elseif ($failedAt -and $authOkAt -and $authOkAt -gt $failedAt) { 'stalled' }
             elseif ($rejectedAt -and (-not $authOkAt -or $authOkAt -lt $rejectedAt)) { 'rejected' }
             elseif ($deniedCount -ge 5) { 'denied' }
             elseif ($failedAt) { 'blocked' }
             else { 'waiting' }
    $faultSince = if ($state -eq 'rejected') { $rejectedSince } elseif ($state -eq 'denied') { $deniedSince } elseif ($state -eq 'blocked') { $failingSince } else { $null }
    # A blocked agent logs a failed login about every 10 minutes but a failed configuration fetch only about hourly.
    $blockedLastAt = if ($jwtFailedAt -and $jwtFailedAt -gt $failedAt) { $jwtFailedAt } else { $failedAt }
    $faultLastAt = if ($state -eq 'rejected') { $rejectedAt } elseif ($state -eq 'denied') { $deniedAt } elseif ($state -eq 'blocked') { $blockedLastAt } else { $null }
    [pscustomobject]@{ State = $state; FailedAt = $failedAt; RejectedSince = $rejectedSince; FaultSince = $faultSince; FaultLastAt = $faultLastAt }
}
# END shared

$stamp = [regex]'^\[(\d{4}-\d\d-\d\dT[\d:.]+[+-]\d\d:\d\d)\]'
$events = New-Object System.Collections.Generic.List[object]
$counts = @{ ok200 = 0; ssl400 = 0; timeout = 0; reset = 0; other = 0 }
$jwt = New-Object System.Collections.Generic.List[object]
$lastOk = $null; $firstFail = $null
$provisioned = $null
$cdn = @{ denied = 0; ok = 0; firstDenied = $null; lastDenied = $null; lastOk = $null }

function Add-Event($at, $what) { $events.Add([pscustomobject]@{ At = $at; What = $what }) }
function Short($dto) { $dto.ToString('HH:mm:ss') }

foreach ($line in [IO.File]::ReadLines($LogPath)) {
    $m = $stamp.Match($line)
    if (-not $m.Success) { continue }
    $at = [datetimeoffset]::Parse($m.Groups[1].Value, [Globalization.CultureInfo]::InvariantCulture)

    if ($line -match 'AgentIsProvisioned current product provisioning state: \[ 3 \]' -and -not $provisioned) {
        $provisioned = $at; Add-Event $at 'provisioned (state 3 Success)'
    }
    elseif ($line -match 'auth::requestOauthToken Oauth token response received\. Status: 200') {
        $counts.ok200++; $lastOk = $at; $jwt.Add([pscustomobject]@{ At = $at; Result = '200 accepted' })
    }
    elseif ($line -match 'auth::fetchJWTWithRetries error occurred when fetching jwt: unexpected status code: 400 .*The SSL certificate error') {
        $counts.ssl400++; if (-not $firstFail) { $firstFail = $at }
        $jwt.Add([pscustomobject]@{ At = $at; Result = '400 The SSL certificate error' })
    }
    elseif ($line -match 'auth::requestOauthToken unable to get jwt: .*Client\.Timeout') {
        $counts.timeout++; if (-not $firstFail) { $firstFail = $at }
        $jwt.Add([pscustomobject]@{ At = $at; Result = 'timeout (10 s)' })
    }
    elseif ($line -match 'auth::requestOauthToken unable to get jwt: .*forcibly closed|auth::requestOauthToken unable to get jwt: .*connection reset') {
        $counts.reset++; if (-not $firstFail) { $firstFail = $at }
        $jwt.Add([pscustomobject]@{ At = $at; Result = 'connection reset by the remote side' })
    }
    elseif ($line -match 'components::doRequestWithRetry received invalid status: ''403''') {
        $cdn.denied++; $cdn.lastDenied = $at; if (-not $cdn.firstDenied) { $cdn.firstDenied = $at }
    }
    elseif ($line -match 'components::Download Successfully downloaded') { $cdn.ok++; $cdn.lastOk = $at }
    elseif ($line -match 'configservice::FetchAllConfigurations failed to fetch configuration data') {
        Add-Event $at 'configuration fetch FAILED'
    }
    elseif ($line -match 'scheduler::Run \(configGUID=Check For Updates\) Executing job') {
        Add-Event $at 'hourly "Check For Updates" runs'
    }
    if ($line -match 'components::Download Successfully downloaded .*(generic-asset-interrogator|osquery|log-collector|software-management-service-requirements)') {
        $name = $Matches[1]
        Add-Event $at "component downloaded: $name"
    }
    elseif ($line -match 'main::stop Msp Agent Service stopping') { Add-Event $at 'service stopping' }
    elseif ($line -match 'main::mainThread Running primary thread') { Add-Event $at 'service running' }
    elseif ($line -match 'messaging::Connect mqtt - connection success') { Add-Event $at 'message broker connected' }
}

# The state is the installer's own (Get-UfaSyncState above); the words say what it means and what the installer does.
$sync = Get-UfaSyncState $LogPath ([datetime]::MinValue)
$meaning = @{
    synced  = 'the inventory component arrived: the data is flowing'
    stalled = 'login works again, nothing delivered: a service restart fetches it now; the installer restarts MspAgent for this, at most twice'
    rejected = 'the cloud refuses the agent''s login; a restart does not help; the installer repairs once after 30 minutes'
    denied  = 'the component CDN answers the agent with 403 and nothing has downloaded since; a plain curl to the CDN works; the installer repairs once after 30 minutes'
    blocked = 'the cloud does not answer the agent''s login yet; often clears by itself; if it lasts the installer restarts MspAgent once after 30 minutes and repairs once after 60 minutes if that did not help'
    waiting = 'no fault, no inventory components yet'
    unknown = 'no log to read'
}
$state = '{0} ({1})' -f $sync.State, $meaning[$sync.State]

$out = New-Object System.Collections.Generic.List[string]
$out.Add("Log: $LogPath")
$out.Add("Computer: $env:COMPUTERNAME    Now: $((Get-Date).ToString('yyyy-MM-dd HH:mm:ss zzz'))")
$out.Add('')
$out.Add("STATE: $state")
if ($sync.FaultSince) { $out.Add(('  failing since {0}, last failure {1} (device time)' -f $sync.FaultSince.ToLocalTime().ToString('HH:mm:ss'), $(if ($sync.FaultLastAt) { $sync.FaultLastAt.ToLocalTime().ToString('HH:mm:ss') } else { '-' }))) }
$out.Add('')
$out.Add('Login-token results (counts):')
$out.Add(('  accepted (200): {0}   400 SSL certificate error: {1}   timeouts: {2}   connection resets: {3}' -f $counts.ok200, $counts.ssl400, $counts.timeout, $counts.reset))
$out.Add(('  first failure: {0}   last accepted: {1}' -f $(if ($firstFail) { Short $firstFail } else { '-' }), $(if ($lastOk) { Short $lastOk } else { 'never' })))
$out.Add('')
$out.Add('Component downloads:')
$out.Add(('  succeeded: {0}   refused with 403 (final failures): {1}   first refusal: {2}   last refusal: {3}   last success: {4}' -f $cdn.ok, $cdn.denied, $(if ($cdn.firstDenied) { Short $cdn.firstDenied } else { '-' }), $(if ($cdn.lastDenied) { Short $cdn.lastDenied } else { '-' }), $(if ($cdn.lastOk) { Short $cdn.lastOk } else { '-' })))
$out.Add('')
$out.Add('Last 12 login attempts:')
foreach ($j in @($jwt | Select-Object -Last 12)) { $out.Add(('  {0}  {1}' -f (Short $j.At), $j.Result)) }
$out.Add('')
$out.Add('Timeline:')
$shown = New-Object System.Collections.Generic.List[string]
foreach ($e in $events) {
    $text = '  {0}  {1}' -f (Short $e.At), $e.What
    # one line per kind per minute keeps long component runs readable
    $key = '{0}|{1}' -f $e.At.ToString('HH:mm'), ($e.What -replace ':.*$', '')
    if ($shown.Contains($key)) { continue }
    $shown.Add($key); $out.Add($text)
}
if ($out.Count -gt 140) { $out = [string[]](@($out[0..19]) + '  ...' + @($out[($out.Count - 100)..($out.Count - 1)])) }

$text = $out -join "`r`n"
Write-Output $text
if ($OutFile) { Set-Content -LiteralPath $OutFile -Value $text -Encoding UTF8 }
