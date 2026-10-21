$ErrorActionPreference = 'Continue'
$pattern = 'N-able|SolarWinds|MspPlatform|Msp Agent|MSP Core|Windows Agent|BeAnywhere|Take Control|Request Handler|File Cache|Patch Management|PME|Ecosystem|AutomationManager|N-central'
$r = [ordered]@{}
$r.uninstall = @(foreach ($view in 'Registry64', 'Registry32') {
    $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey('LocalMachine', $view)
    $u = $base.OpenSubKey('SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall')
    foreach ($n in $u.GetSubKeyNames()) {
        $k = $u.OpenSubKey($n); $name = [string]$k.GetValue('DisplayName'); $pub = [string]$k.GetValue('Publisher')
        if ("$name $pub" -match $pattern) {
            [ordered]@{ view = $view; key = $n; name = $name; version = $k.GetValue('DisplayVersion'); publisher = $pub; uninstall = $k.GetValue('UninstallString'); quiet = $k.GetValue('QuietUninstallString'); location = $k.GetValue('InstallLocation'); msi = [bool]$k.GetValue('WindowsInstaller') }
        }
    }
})
$r.services = @(Get-CimInstance Win32_Service | Where-Object { "$($_.Name) $($_.DisplayName) $($_.PathName)" -match $pattern } | ForEach-Object { '{0} | {1} | {2} | {3}' -f $_.Name, $_.DisplayName, $_.State, $_.PathName })
$roots = @($env:ProgramFiles, ${env:ProgramFiles(x86)}, $env:ProgramData, "$env:windir\Temp", "$env:SystemRoot\System32\config\systemprofile\AppData\Local")
$r.folders = @(foreach ($root in $roots) { if ($root -and (Test-Path $root)) { Get-ChildItem $root -Directory -ErrorAction SilentlyContinue | Where-Object { $_.Name -match $pattern -or $_.Name -match '^(N-able|SolarWinds|Msp|BeAnywhere|MSPScriptExecution)' } | ForEach-Object { '{0} | {1:N1} MB' -f $_.FullName, ((Get-ChildItem $_.FullName -Recurse -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum / 1MB) } } })
$r.registry = @(foreach ($view in 'Registry64', 'Registry32') {
    $sw = [Microsoft.Win32.RegistryKey]::OpenBaseKey('LocalMachine', $view).OpenSubKey('SOFTWARE')
    foreach ($n in $sw.GetSubKeyNames()) { if ($n -match $pattern -or $n -match '^(N-able|SolarWinds|Msp|BeAnywhere)') { "$view HKLM\SOFTWARE\$n : " + (($sw.OpenSubKey($n).GetSubKeyNames()) -join ', ') } }
})
$r.tasks = @(Get-ScheduledTask -ErrorAction SilentlyContinue | Where-Object { "$($_.TaskName) $($_.TaskPath) $($_.Actions.Execute)" -match $pattern } | ForEach-Object { '{0}{1} -> {2}' -f $_.TaskPath, $_.TaskName, ($_.Actions.Execute -join ';') })
$r.certs = @(Get-ChildItem Cert:\LocalMachine\My, Cert:\LocalMachine\Root -ErrorAction SilentlyContinue | Where-Object { $_.Subject -match $pattern -or $_.Issuer -match $pattern } | ForEach-Object { '{0} | {1} | expires {2:yyyy-MM-dd}' -f $_.PSParentPath.Split(':')[-1], $_.Subject, $_.NotAfter })
$Report = $r | ConvertTo-Json -Depth 4 -Compress
Write-Output $Report
