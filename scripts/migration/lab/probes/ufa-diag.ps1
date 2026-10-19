$ErrorActionPreference = 'Continue'
$r = [ordered]@{}
$apps = foreach ($view in 'Registry64', 'Registry32') {
    $u = [Microsoft.Win32.RegistryKey]::OpenBaseKey('LocalMachine', $view).OpenSubKey('SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall')
    foreach ($n in $u.GetSubKeyNames()) {
        $k = $u.OpenSubKey($n)
        $name = $k.GetValue('DisplayName'); $pub = $k.GetValue('Publisher')
        if ($name -and ($pub -match 'N-able|SolarWinds|MSP' -or $name -match 'Agent|N-able|Take Control|MSP|Ecosystem|File Cache|Request Handler|Patch')) {
            '{0} | {1} | {2} | installed {3} | {4}' -f $name, $k.GetValue('DisplayVersion'), $pub, $k.GetValue('InstallDate'), $view
        }
    }
}
$r.apps = @($apps | Sort-Object -Unique)
$r.processes = @(Get-Process | Where-Object { $_.Name -match 'msp|agent|ufa|nable|BASupSrvc|FileCache|RequestHandler' } | ForEach-Object { '{0} pid {1} started {2}' -f $_.Name, $_.Id, $(try { $_.StartTime.ToString('s') } catch { '?' }) })
$r.services = @(Get-Service | Where-Object { $_.DisplayName -match 'Windows Agent|Msp|N-able|Take Control|File Cache|Request Handler|Patch Management' } | ForEach-Object { '{0} ({1}) {2}' -f $_.DisplayName, $_.Name, $_.Status })
$dirs = @("${env:ProgramFiles(x86)}\Msp Agent", "$env:ProgramData\MspPlatform", "$env:ProgramData\N-able Technologies")
$r.dirs = @(foreach ($d in $dirs) { if (Test-Path $d) { Get-ChildItem $d -ErrorAction SilentlyContinue | ForEach-Object { '{0} {1}' -f $_.FullName, $_.LastWriteTime.ToString('s') } } else { "$d (missing)" } })
$r.mspAgentInstallDirTime = if (Test-Path "${env:ProgramFiles(x86)}\Msp Agent") { (Get-Item "${env:ProgramFiles(x86)}\Msp Agent").CreationTime.ToString('s') } else { '-' }
$r.windowsAgentDirTime = if (Test-Path "${env:ProgramFiles(x86)}\N-able Technologies\Windows Agent") { (Get-Item "${env:ProgramFiles(x86)}\N-able Technologies\Windows Agent").CreationTime.ToString('s') } else { '-' }
$Report = ($r | ConvertTo-Json -Depth 3 -Compress)
Write-Output $Report
