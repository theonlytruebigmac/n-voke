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

$stamp = [regex]'^\[(\d{4}-\d\d-\d\dT[\d:.]+[+-]\d\d:\d\d)\]'
$events = New-Object System.Collections.Generic.List[object]
$counts = @{ ok200 = 0; ssl400 = 0; timeout = 0; reset = 0; other = 0 }
$jwt = New-Object System.Collections.Generic.List[object]
$lastOk = $null; $firstFail = $null
$lastSslAt = $null; $lastFailedFetch = $null; $synced = $null; $provisioned = $null

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
        $counts.ssl400++; $lastSslAt = $at; if (-not $firstFail) { $firstFail = $at }
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
    elseif ($line -match 'configservice::FetchAllConfigurations failed to fetch configuration data') {
        $lastFailedFetch = $at; Add-Event $at 'configuration fetch FAILED'
    }
    elseif ($line -match 'scheduler::Run \(configGUID=Check For Updates\) Executing job') {
        Add-Event $at 'hourly "Check For Updates" runs'
    }
    elseif ($line -match 'components::Download Successfully downloaded .*(generic-asset-interrogator|osquery|log-collector|software-management-service-requirements)') {
        $name = $Matches[1]
        if ($name -eq 'generic-asset-interrogator' -and -not $synced) { $synced = $at }
        Add-Event $at "component downloaded: $name"
    }
    elseif ($line -match 'main::stop Msp Agent Service stopping') { Add-Event $at 'service stopping' }
    elseif ($line -match 'main::mainThread Running primary thread') { Add-Event $at 'service running' }
    elseif ($line -match 'messaging::Connect mqtt - connection success') { Add-Event $at 'message broker connected' }
}

# Same rules as the installer's Get-UfaSyncState.
$state =
    if ($synced -and (-not $lastFailedFetch -or $synced -gt $lastFailedFetch)) { 'synced' }
    elseif ($lastFailedFetch -and $lastOk -and $lastOk -gt $lastFailedFetch) { 'stalled (login works again, nothing delivered: a service restart fetches it now)' }
    elseif ($lastSslAt -and (-not $lastOk -or $lastOk -lt $lastSslAt)) { 'rejected (the cloud refuses the agent''s login; a restart does not help)' }
    elseif ($lastFailedFetch) { 'blocked (the cloud does not answer yet)' }
    else { 'waiting (no fault, no inventory components yet)' }

$out = New-Object System.Collections.Generic.List[string]
$out.Add("Log: $LogPath")
$out.Add("Computer: $env:COMPUTERNAME    Now: $((Get-Date).ToString('yyyy-MM-dd HH:mm:ss zzz'))")
$out.Add('')
$out.Add("STATE: $state")
$out.Add('')
$out.Add('Login-token results (counts):')
$out.Add(('  accepted (200): {0}   400 SSL certificate error: {1}   timeouts: {2}   connection resets: {3}' -f $counts.ok200, $counts.ssl400, $counts.timeout, $counts.reset))
$out.Add(('  first failure: {0}   last accepted: {1}' -f $(if ($firstFail) { Short $firstFail } else { '-' }), $(if ($lastOk) { Short $lastOk } else { 'never' })))
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
