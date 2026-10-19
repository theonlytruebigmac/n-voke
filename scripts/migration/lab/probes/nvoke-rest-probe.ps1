<#
.SYNOPSIS
    N-voke migration Phase 0: REST-path probe. Safe to run; changes nothing.
.DESCRIPTION
    Reports how arguments from the direct task's CommandLine parameter arrive,
    the account and bitness it runs under, its parent processes, and prints one
    NVOKE-RESULT {json} line. Delete it from the repository after testing.
#>
param(
    [string]$Action = 'Probe',
    [string]$DestServer = '-',
    [string]$RegistrationToken = '-',
    [string]$TestQuote = '-',
    [int]$TestInt = 0,
    [switch]$TestSwitch,
    [int]$TestExit = 0,
    [ValidateSet('exit', 'setshouldexit', 'none')]
    [string]$ExitMode = 'exit',
    [int]$LongOutputChars = 0,
    [switch]$WriteStderr,
    [Parameter(ValueFromRemainingArguments = $true)]
    $Remaining
)

$ErrorActionPreference = 'Continue'
$r = [ordered]@{
    action            = $Action
    destServer        = $DestServer
    registrationToken = if ($RegistrationToken -ne '-') { "<$($RegistrationToken.Length) chars>" } else { '-' }
    testQuote         = $TestQuote
    testInt           = $TestInt
    testIntType       = $TestInt.GetType().Name
    testSwitch        = [bool]$TestSwitch
    remaining         = @($Remaining)
    whoami            = [Security.Principal.WindowsIdentity]::GetCurrent().Name
    psVersion         = $PSVersionTable.PSVersion.ToString()
    is64BitProcess    = [Environment]::Is64BitProcess
    psCommandPath     = $PSCommandPath
    pwd               = (Get-Location).Path
    executionPolicy   = (Get-ExecutionPolicy).ToString()
}

# The full command line shows whether the token is visible in the process list.
$self = Get-CimInstance Win32_Process -Filter "ProcessId=$PID"
$r.commandLine = if ($RegistrationToken -ne '-') { $self.CommandLine.Replace($RegistrationToken, '<TOKEN>') } else { $self.CommandLine }
$r.commandLineContainsToken = ($RegistrationToken -ne '-') -and $self.CommandLine.Contains($RegistrationToken)

$chain = @()
$p = Get-CimInstance Win32_Process -Filter "ProcessId=$($self.ParentProcessId)"
for ($i = 0; $i -lt 5 -and $p; $i++) {
    $chain += '{0} {1} :: {2}' -f $p.ProcessId, $p.Name, $p.ExecutablePath
    $p = Get-CimInstance Win32_Process -Filter "ProcessId=$($p.ParentProcessId)"
}
$r.parentChain = $chain

Write-Output 'CHANNEL-1 Write-Output'
Write-Host 'CHANNEL-2 Write-Host'
[Console]::Out.WriteLine('CHANNEL-3 Console.Out')
Write-Output ('PROBE-JSON ' + ($r | ConvertTo-Json -Depth 3 -Compress))
if ($WriteStderr) {
    [Console]::Error.WriteLine('CHANNEL-4 Console.Error')
    Write-Error 'CHANNEL-5 Write-Error'
}
if ($LongOutputChars -gt 0) {
    Write-Output ('LONG:' + ('x' * $LongOutputChars) + ':END')
}

$result = [ordered]@{ result = if ($TestExit -eq 0) { 'ok' } else { 'failed' }; errorCode = $TestExit; launcherVersion = 'phase0-rest-probe-v2'; taskCreated = $false }
Write-Output ('NVOKE-RESULT ' + ($result | ConvertTo-Json -Compress))

switch ($ExitMode) {
    'exit' { exit $TestExit }
    'setshouldexit' { if ($TestExit -ne 0) { $host.SetShouldExit($TestExit) } }
    'none' { }
}
