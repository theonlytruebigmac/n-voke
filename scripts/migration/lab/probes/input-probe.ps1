$ErrorActionPreference = 'Continue'
$r = [ordered]@{}
$r.whoami = [Security.Principal.WindowsIdentity]::GetCurrent().Name
$r.psVersion = $PSVersionTable.PSVersion.ToString()
$r.psEdition = $PSVersionTable.PSEdition
$r.is64BitProcess = [Environment]::Is64BitProcess
$r.psCommandPath = $PSCommandPath
$r.invocationLine = $MyInvocation.Line
$r.invocationName = $MyInvocation.InvocationName
$r.args = @($args)
$r.boundParams = @{} + $PSBoundParameters
$r.pwd = (Get-Location).Path
$r.executionPolicy = (Get-ExecutionPolicy).ToString()
$r.languageMode = $ExecutionContext.SessionState.LanguageMode.ToString()

foreach ($n in 'TestString', 'TestPassword', 'TestQuote', 'TestInt', 'TestBool', 'TestExit') {
    $v = Get-Variable -Name $n -ErrorAction SilentlyContinue
    $r["var_$n"] = if ($v) { '{0} [{1}] scope-found' -f $v.Value, $(if ($null -ne $v.Value) { $v.Value.GetType().Name } else { 'null' }) } else { $null }
    $r["env_$n"] = [Environment]::GetEnvironmentVariable($n)
}
$r.envMatches = @(Get-ChildItem env: | Where-Object { $_.Name -match 'test|input|nable|ufa|script|output' } | ForEach-Object { "$($_.Name)=$($_.Value)" })
$r.userVariables = @(Get-Variable | Where-Object { $_.Name -match '^(Test|Out|Input|Param|Nable|Ufa)' } | ForEach-Object { $_.Name })

$chain = @()
$p = Get-CimInstance Win32_Process -Filter "ProcessId=$PID"
for ($i = 0; $i -lt 6 -and $p; $i++) {
    $chain += '{0} {1} :: {2}' -f $p.ProcessId, $p.Name, $p.CommandLine
    $p = Get-CimInstance Win32_Process -Filter "ProcessId=$($p.ParentProcessId)"
}
$r.processChain = $chain

if ($PSCommandPath -and (Test-Path $PSCommandPath)) {
    $all = Get-Content $PSCommandPath
    $r.scriptLineCount = $all.Count
    $r.scriptHead = ($all | Select-Object -First 30) -join "`n"
    $r.scriptTail = ($all | Select-Object -Last 15) -join "`n"
}

Write-Output '=== NVOKE-PROBE BEGIN ==='
Write-Output ($r | ConvertTo-Json -Depth 4)
Write-Output '=== NVOKE-PROBE END ==='
Write-Error 'probe: this line goes to stderr'

$OutVar = 'set-as-script-variable'
$global:OutGlobal = 'set-as-global-variable'
$env:OutEnv = 'set-as-env-variable'
$OutBool = $true
Write-Output 'OutStd=set-via-stdout'

$code = 0
if (Get-Variable -Name TestExit -ErrorAction SilentlyContinue) { $code = [int]$TestExit }
exit $code
