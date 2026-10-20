$ErrorActionPreference = 'Continue'
$root = [Environment]::GetFolderPath('CommonApplicationData') + '\N-voke\Migration'
$r = [ordered]@{}
$r.tasks = @(Get-ScheduledTask -TaskName 'N-voke*' -ErrorAction SilentlyContinue | ForEach-Object {
    $i = $_ | Get-ScheduledTaskInfo
    '{0} | {1} | next {2} | last {3} ({4}) | trigger {5}' -f $_.TaskName, $_.State, $i.NextRunTime, $i.LastRunTime, $i.LastTaskResult, (($_.Triggers | ForEach-Object { $_.StartBoundary }) -join ', ')
})
$r.folderExists = Test-Path $root
$r.files = @(if (Test-Path $root) { Get-ChildItem $root -Recurse -File | ForEach-Object { '{0} | {1} B' -f $_.FullName.Substring($root.Length), $_.Length } })
$r.jobJsonPresent = Test-Path "$root\job.json"
$Report = $r | ConvertTo-Json -Compress
Write-Output $Report
