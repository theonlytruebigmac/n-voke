# Checks the endpoint scripts: PSScriptAnalyzer, then the Pester tests (which build the script first).
# Usage: pwsh -File run-tests.ps1          (or powershell.exe -File run-tests.ps1 on Windows)
# Needs Pester 5+ and PSScriptAnalyzer: Install-Module Pester, PSScriptAnalyzer -Scope CurrentUser
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot

$findings = @(Invoke-ScriptAnalyzer -Path $here -Recurse -Settings (Join-Path $here 'PSScriptAnalyzerSettings.psd1') |
    Where-Object { $_.ScriptName -notlike '*.Tests.ps1' })
if ($findings) {
    $findings | Format-Table RuleName, Severity, ScriptName, Line, Message -AutoSize -Wrap | Out-String -Width 200 | Write-Output
    Write-Output "PSScriptAnalyzer: $($findings.Count) finding(s)."
} else {
    Write-Output 'PSScriptAnalyzer: clean.'
}

Import-Module Pester -MinimumVersion 5.0.0
$config = New-PesterConfiguration
$config.Run.Path = $here
$config.Run.PassThru = $true
$config.Output.Verbosity = 'Normal'
$result = Invoke-Pester -Configuration $config

if ($findings -or $result.FailedCount -gt 0) { exit 1 }
