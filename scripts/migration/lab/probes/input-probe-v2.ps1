$ErrorActionPreference = 'Continue'
$dir = Split-Path -Parent $PSCommandPath
Write-Output 'SCRIPT_DIR_FILES:'
Get-ChildItem $dir | Sort-Object LastWriteTime | ForEach-Object { '  {0} {1} {2}' -f $_.Name, $_.LastWriteTime.ToString('o'), $_.Length }
$old = @(Get-ChildItem $dir -Filter *.ps1 | Where-Object { $_.FullName -ne $PSCommandPath })
$leak = @($old | Where-Object { Select-String -Path $_.FullName -SimpleMatch 'fake-s3cret-not-real' -Quiet })
Write-Output "OLD_PS1_FILES: $($old.Count); WITH_TEST_SECRET: $($leak.Count)"
Write-Output "SYSNATIVE_POWERSHELL: $(Test-Path "$env:windir\sysnative\WindowsPowerShell\v1.0\powershell.exe")"
Write-Output "SCHEDULEDTASKS_MODULE: $([bool](Get-Command Register-ScheduledTask -ErrorAction SilentlyContinue))"

$OutVar = 'single line value with spaces'
if ($TestBool) { $OutGlobal = "line1`nline2" } else { $OutGlobal = 'no newline here' }
$OutEnv = (@{ a = 1; b = 'x y' } | ConvertTo-Json -Compress)
$OutStd = ''
$OutBool = $true
if ($TestExit -ne 0) { $host.SetShouldExit([int]$TestExit) }
