# N-voke agent migration: endpoint script (Launch, Status, Cleanup). Version __SCRIPT_VERSION__.
#
# One file for both delivery paths:
#  - Ecoverse assigns the input variables above this script, so there is no param() block
#    (param() must be the first statement). Outputs are read from script variables after
#    the script ends: never call exit, never leave an output blank or multi-line.
#  - The N-central REST path passes "-Name value" arguments, parsed from $args below. The
#    result is printed as one NVOKE-RESULT {json} line. Nothing is written to stderr,
#    because stderr replaces stdout in N-central's task output.
# Failure is reported with $host.SetShouldExit(code) as the very last statement.

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$ScriptVersion = '__SCRIPT_VERSION__'

# ---- inputs -------------------------------------------------------------------
for ($i = 0; $i -lt $args.Count; $i++) {
    if ([string]$args[$i] -match '^-([A-Za-z]\w*)$') {
        $argName = $Matches[1]
        if ($i + 1 -lt $args.Count -and [string]$args[$i + 1] -notmatch '^-[A-Za-z]\w*$') { $i++; $argValue = [string]$args[$i] } else { $argValue = 'true' }
        Set-Variable -Name $argName -Value $argValue -Scope Script
    }
}
if (-not $Action) { $Action = 'Launch' }
if (-not $TaskRepeatMinutes) { $TaskRepeatMinutes = 15 }
if (-not $TaskExpiryDays) { $TaskExpiryDays = 14 }
if (-not $CleanMode -or $CleanMode -eq '-') { $CleanMode = 'full' }

# ---- outputs (every one always has a single-line value) -----------------------
$Result = 'failed'
$ErrorCode = '0'
$Message = '-'
$TaskCreated = 'false'
$StagedAt = '-'
$StatusJson = '-'
$TaskState = '-'
$Cleaned = 'false'

$Root = [Environment]::GetFolderPath('CommonApplicationData') + '\N-voke\Migration'
$TaskName = 'N-voke Agent Migration'
$InstallerPath = $Root + '\installAgent.ps1'
$InstallerB64 = '__INSTALLER_B64__'
$InstallerSha256 = '__INSTALLER_SHA256__'
$ServerConfigPath = [Environment]::GetFolderPath('ProgramFilesX86') + '\N-able Technologies\Windows Agent\config\ServerConfig.xml'

function Get-OneLine($Value, [int]$Max = 4000) {
    $s = [string]$Value
    if ([string]::IsNullOrWhiteSpace($s)) { return '-' }
    $s = ($s -replace '[\r\n]+', ' | ').Trim()
    if ($s.Length -gt $Max) { $s = $s.Substring(0, $Max) + '...' }
    return $s
}

function Set-RestrictedAcl([string]$Path) {
    $acl = New-Object System.Security.AccessControl.DirectorySecurity
    $acl.SetAccessRuleProtection($true, $false)
    foreach ($sid in 'S-1-5-18', 'S-1-5-32-544') {
        $id = New-Object System.Security.Principal.SecurityIdentifier $sid
        $acl.AddAccessRule((New-Object System.Security.AccessControl.FileSystemAccessRule($id, 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow')))
    }
    Set-Acl -Path $Path -AclObject $acl
}

function Get-StatusSummary {
    $statusPath = Join-Path $Root 'status.json'
    if (-not (Test-Path $statusPath)) { return '-' }
    $s = Get-Content $statusPath -Raw | ConvertFrom-Json
    $s.history = @($s.history | Select-Object -Last 30)
    return ($s | ConvertTo-Json -Depth 5 -Compress)
}

function Get-TaskSummary {
    $task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    if (-not $task) { return 'absent' }
    $info = $task | Get-ScheduledTaskInfo
    return ('{0}; last run {1}; last result {2}; next run {3}' -f $task.State, $info.LastRunTime, $info.LastTaskResult, $info.NextRunTime)
}

function Invoke-Launch {
    if ($PSVersionTable.PSVersion -lt [version]'5.1') { throw [System.ArgumentException] "PowerShell $($PSVersionTable.PSVersion) is too old (5.1 or newer needed)" }
    if ([string]$DestServer -notmatch '^[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?(\.[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?)+$') { throw [System.ArgumentException] "DestServer '$DestServer' is not a host name" }
    if ([string]$DestCustomerId -notmatch '^\d+$') { throw [System.ArgumentException] "DestCustomerId '$DestCustomerId' is not numeric" }
    if ([string]$RegistrationToken -notmatch '^[A-Za-z0-9-]{8,128}$') { throw [System.ArgumentException] 'RegistrationToken is missing or malformed' }
    if ($CleanMode -notin 'full', 'none') { throw [System.ArgumentException] "CleanMode '$CleanMode' must be full or none" }
    if ($LauncherVersion -and $LauncherVersion -ne '-' -and $LauncherVersion -ne $ScriptVersion) { throw [System.ArgumentException] "N-voke expected script $LauncherVersion but this is ${ScriptVersion}: re-publish or re-upload the script" }

    New-Item -ItemType Directory -Path $Root -Force | Out-Null
    Set-RestrictedAcl (Split-Path $Root -Parent)
    Set-RestrictedAcl $Root

    $bytes = [Convert]::FromBase64String($InstallerB64)
    [IO.File]::WriteAllBytes($InstallerPath, $bytes)
    $hash = (Get-FileHash -Path $InstallerPath -Algorithm SHA256).Hash
    if ($hash -ne $InstallerSha256) { throw "installAgent.ps1 hash mismatch ($hash)" }

    $currentServer = $null
    if (Test-Path $ServerConfigPath) { try { $currentServer = ([xml](Get-Content $ServerConfigPath -Raw)).SelectSingleNode('//ServerIP').InnerText } catch { } }
    $job = [ordered]@{
        schema            = 1
        migrationId       = Get-OneLine $MigrationId 200
        launcherVersion   = $ScriptVersion
        mode              = if ($currentServer -and $currentServer -eq $DestServer) { 'reinstall' } else { 'move' }
        sourceServer      = $currentServer
        destServer        = $DestServer
        destCustomerId    = $DestCustomerId
        cleanMode         = $CleanMode
        registrationToken = $RegistrationToken
        createdAt         = (Get-Date).ToUniversalTime().ToString('o')
    }
    $job | ConvertTo-Json | Set-Content -Path (Join-Path $Root 'job.json') -Encoding UTF8

    $stagedAt = (Get-Date).ToUniversalTime().ToString('o')
    $status = [ordered]@{
        schema = 1; migrationId = $job.migrationId; launcherVersion = $ScriptVersion; mode = $job.mode
        sourceServer = $currentServer; destServer = $DestServer; destCustomerId = $DestCustomerId
        stage = 'staged'; stagedAt = $stagedAt; attempts = 0
        history = @([ordered]@{ at = $stagedAt; event = 'staged'; detail = "mode $($job.mode), clean $CleanMode, from $currentServer" })
    }
    $status | ConvertTo-Json -Depth 5 | Set-Content -Path (Join-Path $Root 'status.json') -Encoding UTF8

    # The action runs 64-bit PowerShell: Task Scheduler resolves System32 itself, even when this launcher is 32-bit.
    $ps = Join-Path $env:windir 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $action = New-ScheduledTaskAction -Execute $ps -Argument "-NoProfile -NoLogo -NonInteractive -ExecutionPolicy Bypass -File `"$InstallerPath`""
    $end = (Get-Date).AddDays([int]$TaskExpiryDays).ToString('s')
    $repeat = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(2) -RepetitionInterval (New-TimeSpan -Minutes ([int]$TaskRepeatMinutes)) -RepetitionDuration (New-TimeSpan -Days ([int]$TaskExpiryDays))
    $repeat.EndBoundary = $end
    $boot = New-ScheduledTaskTrigger -AtStartup
    $boot.Delay = 'PT2M'
    $boot.EndBoundary = $end
    $settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Hours 2) -MultipleInstances IgnoreNew -StartWhenAvailable -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -DeleteExpiredTaskAfter (New-TimeSpan -Days 1)
    $principal = New-ScheduledTaskPrincipal -UserId 'S-1-5-18' -LogonType ServiceAccount -RunLevel Highest
    Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger @($repeat, $boot) -Settings $settings -Principal $principal -Description "N-voke agent migration $($job.migrationId) to $DestServer" -Force | Out-Null

    $script:TaskState = Get-TaskSummary
    if ($script:TaskState -eq 'absent') { throw 'The scheduled task was not created' }
    $script:TaskCreated = 'true'
    $script:StagedAt = $stagedAt
    $script:StatusJson = Get-StatusSummary
    $script:Result = 'staged'
    $script:Message = "Staged: scheduled task registered, mode $($job.mode), first run in 2 minutes"
}

function Invoke-Status {
    $script:StatusJson = Get-StatusSummary
    $script:TaskState = Get-TaskSummary
    $log = Get-ChildItem (Join-Path $Root 'logs') -Filter 'install-*.log' -ErrorAction SilentlyContinue | Sort-Object Name -Descending | Select-Object -First 1
    $tail = if ($log) { (Get-Content $log.FullName -Tail 15) -join ' | ' } else { 'no install log yet' }
    $script:Result = 'ok'
    $script:Message = Get-OneLine "Last log: $tail" 1500
}

function Invoke-Cleanup {
    $script:StatusJson = Get-StatusSummary
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
    if (Test-Path $Root) { Remove-Item -Path $Root -Recurse -Force }
    $script:TaskState = Get-TaskSummary
    $script:Cleaned = if ((-not (Test-Path $Root)) -and $script:TaskState -eq 'absent') { 'true' } else { 'false' }
    $script:Result = if ($script:Cleaned -eq 'true') { 'cleaned' } else { 'failed' }
    $script:Message = if ($script:Cleaned -eq 'true') { 'Scheduled task and folder removed' } else { "Left behind: task $($script:TaskState); folder exists: $(Test-Path $Root)" }
}

try {
    switch ($Action) {
        'Launch' { Invoke-Launch }
        'Status' { Invoke-Status }
        'Cleanup' { Invoke-Cleanup }
        default { throw [System.ArgumentException] "Unknown Action '$Action'" }
    }
} catch [System.ArgumentException] {
    $Result = 'failed'; $ErrorCode = '1004'; $Message = $_.Exception.Message
} catch [System.UnauthorizedAccessException] {
    $Result = 'failed'; $ErrorCode = '1005'; $Message = $_.Exception.Message
} catch {
    $Result = 'failed'; $ErrorCode = '1005'; $Message = "$($_.Exception.GetType().Name): $($_.Exception.Message)"
}
if ($Result -eq 'failed' -and $ErrorCode -eq '0') { $ErrorCode = '1005' }

$Result = Get-OneLine $Result
$ErrorCode = Get-OneLine $ErrorCode
$Message = Get-OneLine $Message 1500
$TaskCreated = Get-OneLine $TaskCreated
$StagedAt = Get-OneLine $StagedAt
$StatusJson = Get-OneLine $StatusJson 16000
$TaskState = Get-OneLine $TaskState 500
$Cleaned = Get-OneLine $Cleaned
$ScriptVersion = Get-OneLine $ScriptVersion

Write-Output ('NVOKE-RESULT ' + ([ordered]@{ action = $Action; result = $Result; errorCode = [int]$ErrorCode; message = $Message; scriptVersion = $ScriptVersion; taskCreated = $TaskCreated -eq 'true'; stagedAt = $StagedAt; taskState = $TaskState; cleaned = $Cleaned -eq 'true'; statusJson = $StatusJson } | ConvertTo-Json -Compress))
if ([int]$ErrorCode -ne 0) { $host.SetShouldExit([int]$ErrorCode) }
