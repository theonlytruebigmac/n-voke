#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0.0' }
# Tests for the endpoint scripts. Run with: pwsh -File run-tests.ps1   (or Invoke-Pester)
#
# The built script is run as a child process, the way Ecoverse and N-central run it, so these check
# what a caller actually sees: the NVOKE-RESULT line, the exit code, and that nothing goes to stderr.
# Functions are lifted out of the source files with the parser instead of running the scripts, which
# do their work at the top level.

BeforeDiscovery {
    $onWindows = ($PSVersionTable.PSVersion.Major -lt 6) -or $IsWindows
}

BeforeAll {
    $here = $PSScriptRoot
    $hostPath = (Get-Process -Id $PID).Path

    & node (Join-Path $here 'build.mjs') | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'build.mjs failed' }
    $dist = Join-Path $here 'dist'
    $built = Join-Path $dist 'nvoke-migration.ps1'

    # Defines the named functions from a script file in the caller's scope, without running the script.
    function Get-FunctionText([string]$Path, [string[]]$Names) {
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($Path, [ref]$null, [ref]$null)
        $found = $ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $Names -contains $n.Name }, $true)
        if (@($found).Count -ne $Names.Count) { throw "Expected functions $($Names -join ', ') in $Path" }
        ($found | ForEach-Object { $_.Extent.Text }) -join "`n"
    }

    # Runs the built script as a child process. $Inputs go in as "-Name value" arguments (the REST path),
    # or as variable assignments above the script (the Ecoverse path).
    function Invoke-Endpoint([hashtable]$Inputs, [switch]$EcoverseForm) {
        if ($EcoverseForm) {
            $assignments = ($Inputs.GetEnumerator() | ForEach-Object { '${0} = ''{1}''' -f $_.Key, ($_.Value -replace "'", "''") }) -join "`r`n"
            $file = Join-Path ([IO.Path]::GetTempPath()) ('nvoke-eco-' + [guid]::NewGuid().ToString('N') + '.ps1')
            Set-Content -Path $file -Value ($assignments + "`r`n" + (Get-Content -Raw $built)) -Encoding Ascii
            $arguments = @('-NoProfile', '-File', $file)
        } else {
            $file = $null
            $arguments = @('-NoProfile', '-File', $built)
            # Windows PowerShell 5.1 joins -ArgumentList with spaces and quotes nothing, so a value with a space would split into several arguments.
            foreach ($key in $Inputs.Keys) { $value = [string]$Inputs[$key]; if ($value -match '\s') { $value = '"' + $value + '"' }; $arguments += "-$key"; $arguments += $value }
        }
        $out = [IO.Path]::GetTempFileName(); $err = [IO.Path]::GetTempFileName()
        try {
            $p = Start-Process -FilePath $hostPath -ArgumentList $arguments -Wait -PassThru -NoNewWindow -RedirectStandardOutput $out -RedirectStandardError $err
            $stdout = @(Get-Content $out)
            $line = @($stdout | Where-Object { $_ -like 'NVOKE-RESULT *' })
            [pscustomobject]@{
                ExitCode = $p.ExitCode
                Stdout   = $stdout
                Stderr   = (Get-Content $err -Raw)
                Lines    = $line.Count
                Result   = if ($line.Count -eq 1) { $line[0].Substring('NVOKE-RESULT '.Length) | ConvertFrom-Json } else { $null }
            }
        } finally {
            Remove-Item $out, $err -Force -ErrorAction SilentlyContinue
            if ($file) { Remove-Item $file -Force -ErrorAction SilentlyContinue }
        }
    }

    # Windows keeps the whole exit code; Linux and macOS keep only its low 8 bits.
    $exit1004 = if (($PSVersionTable.PSVersion.Major -lt 6) -or $IsWindows) { 1004 } else { 1004 -band 255 }

    $good = @{ Action = 'Launch'; DestServer = 'new.example.com'; DestCustomerId = '1234'; RegistrationToken = 'abcd-1234-efgh-5678'; MigrationId = 'mig_1/wave-1' }
}

Describe 'the built script' {
    It 'is ASCII, because Windows PowerShell 5.1 reads a file without a byte-order mark as ANSI' {
        $bytes = [IO.File]::ReadAllBytes($built)
        ($bytes | Where-Object { $_ -gt 126 -or ($_ -lt 32 -and $_ -notin 9, 10, 13) }).Count | Should -Be 0
    }

    It 'parses with no errors' {
        $errors = $null
        [void][System.Management.Automation.Language.Parser]::ParseFile($built, [ref]$null, [ref]$errors)
        $errors.Count | Should -Be 0
    }

    It 'has no param() block, which Ecoverse''s input assignments would break' {
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($built, [ref]$null, [ref]$null)
        $ast.ParamBlock | Should -BeNullOrEmpty
    }

    It 'never calls exit, which would lose Ecoverse''s outputs' {
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($built, [ref]$null, [ref]$null)
        $exits = $ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.ExitStatementAst] }, $true)
        @($exits).Count | Should -Be 0
    }

    It 'has its version stamped in and no placeholders left' {
        $text = Get-Content -Raw $built
        $text | Should -Not -Match '__[A-Z0-9_]+__'
        $text | Should -Match "ScriptVersion = '0\.1\.0\+[0-9a-f]{7}'"
        (Get-Content -Raw (Join-Path $dist 'version.json') | ConvertFrom-Json).version | Should -Match '^0\.1\.0\+[0-9a-f]{7}$'
    }

    It 'embeds the installer with the SHA-256 it announces' {
        $text = Get-Content -Raw $built
        $b64 = [regex]::Match($text, "\`$InstallerB64 = '([^']+)'").Groups[1].Value
        $sha = [regex]::Match($text, "\`$InstallerSha256 = '([0-9A-F]{64})'").Groups[1].Value
        $bytes = [Convert]::FromBase64String($b64)
        $hash = [BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash($bytes)).Replace('-', '')
        $hash | Should -Be $sha
        $bytes.Length | Should -Be ([IO.File]::ReadAllBytes((Join-Path $here 'installAgent.ps1'))).Length
    }
}

Describe 'a failed launch' {
    It 'reports <Why> as exactly one NVOKE-RESULT line, error 1004, exit code 1004 and nothing on stderr' -ForEach @(
        @{ Why = 'a bad host name'; Change = @{ DestServer = 'https://new.example.com/x' }; Says = 'DestServer' }
        @{ Why = 'a host with a port'; Change = @{ DestServer = 'new.example.com:8443' }; Says = 'DestServer' }
        @{ Why = 'a non-numeric customer ID'; Change = @{ DestCustomerId = '12a' }; Says = 'DestCustomerId' }
        @{ Why = 'a malformed token'; Change = @{ RegistrationToken = 'short' }; Says = 'RegistrationToken' }
        @{ Why = 'a token with a shell character'; Change = @{ RegistrationToken = 'abcd1234&calc' }; Says = 'RegistrationToken' }
        @{ Why = 'a bad ReviewDays'; Change = @{ ReviewDays = '-1' }; Says = 'ReviewDays' }
        @{ Why = 'an unknown CleanMode'; Change = @{ CleanMode = 'everything' }; Says = 'CleanMode' }
        @{ Why = 'a bad ExpectUfa'; Change = @{ ExpectUfa = 'maybe' }; Says = 'ExpectUfa' }
        @{ Why = 'a launcher version that is not this script'; Change = @{ LauncherVersion = '9.9.9+0000000' }; Says = 'expected script' }
        @{ Why = 'an unknown action'; Change = @{ Action = 'Explode' }; Says = 'Unknown Action' }
    ) {
        $inputs = $good.Clone()
        foreach ($k in $Change.Keys) { $inputs[$k] = $Change[$k] }
        $r = Invoke-Endpoint $inputs
        $r.Lines | Should -Be 1
        $r.Result.result | Should -Be 'failed'
        $r.Result.errorCode | Should -Be 1004
        $r.Result.taskCreated | Should -BeFalse
        $r.Result.message | Should -Match $Says
        $r.ExitCode | Should -Be $exit1004
        [string]$r.Stderr | Should -BeNullOrEmpty
    }

    It 'never prints the registration token, even when something else is wrong' {
        $inputs = $good.Clone()
        $inputs.DestServer = 'not a host'
        $r = Invoke-Endpoint $inputs
        ($r.Stdout -join "`n") | Should -Not -Match 'abcd-1234-efgh-5678'
        [string]$r.Stderr | Should -Not -Match 'abcd-1234-efgh-5678'
    }

    It 'behaves the same when inputs arrive as variables above the script, as in Ecoverse' {
        $inputs = $good.Clone()
        $inputs.DestServer = 'not a host'
        $inputs.TaskRepeatMinutes = '15'; $inputs.TaskExpiryDays = '14'; $inputs.CleanMode = 'full'; $inputs.ExpectUfa = 'false'; $inputs.ReviewDays = '7'; $inputs.LauncherVersion = '-'
        $r = Invoke-Endpoint $inputs -EcoverseForm
        $r.Lines | Should -Be 1
        $r.Result.errorCode | Should -Be 1004
        $r.ExitCode | Should -Be $exit1004
        [string]$r.Stderr | Should -BeNullOrEmpty
    }

    It 'treats a placeholder of - for an unused input as the default, as Ecoverse sends' {
        $inputs = $good.Clone()
        $inputs.DestServer = 'not a host'
        $inputs.CleanMode = '-'; $inputs.ExpectUfa = '-'; $inputs.ReviewDays = '-'; $inputs.LauncherVersion = '-'
        # Sent the way Ecoverse sends them: as variables set before the script, not as command-line arguments. Windows PowerShell 5.1
        # doesn't take a lone - as a parameter value on the command line.
        $r = Invoke-Endpoint $inputs -EcoverseForm
        $r.Result.message | Should -Match 'DestServer' -Because ("stdout: " + ($r.Stdout -join ' | ') + "; stderr: " + $r.Stderr)
    }
}

Describe 'Get-OneLine' {
    BeforeAll { . ([scriptblock]::Create((Get-FunctionText $built @('Get-OneLine')))) }

    It 'is never blank, because Ecoverse drops a blank output and fails the whole set' {
        Get-OneLine $null | Should -Be '-'
        Get-OneLine '' | Should -Be '-'
        Get-OneLine "  `r`n " | Should -Be '-'
    }
    It 'joins lines, because a line break shifts every later output into the wrong variable' {
        Get-OneLine "a`r`nb`nc" | Should -Be 'a | b | c'
    }
    It 'shortens long values' {
        (Get-OneLine ('x' * 100) 10) | Should -Be ('x' * 10 + '...')
    }
}

Describe 'Get-StatusSummary' {
    BeforeAll {
        . ([scriptblock]::Create((Get-FunctionText $built @('Get-StatusSummary'))))
        $Root = Join-Path ([IO.Path]::GetTempPath()) ('nvoke-test-' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $Root | Out-Null
    }
    AfterAll { Remove-Item $Root -Recurse -Force -ErrorAction SilentlyContinue }

    It 'is - when there is no status file' {
        Get-StatusSummary | Should -Be '-'
    }
    It 'returns compact one-line JSON with only the last 30 history entries' {
        $history = 1..45 | ForEach-Object { @{ at = "t$_"; event = 'e'; detail = "d$_" } }
        @{ schema = 1; stage = 'healthy'; history = $history } | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $Root 'status.json')
        $summary = Get-StatusSummary
        $summary | Should -Not -Match "[\r\n]"
        $parsed = $summary | ConvertFrom-Json
        $parsed.stage | Should -Be 'healthy'
        @($parsed.history).Count | Should -Be 30
        $parsed.history[0].detail | Should -Be 'd16'
        $parsed.history[29].detail | Should -Be 'd45'
    }
}

Describe 'Set-RestrictedAcl' -Skip:(-not $onWindows) {
    BeforeAll { . ([scriptblock]::Create((Get-FunctionText $built @('New-RestrictedSecurity', 'Set-RestrictedAcl')))) }

    It 'leaves only SYSTEM and Administrators, with inheritance off' {
        $dir = Join-Path ([IO.Path]::GetTempPath()) ('nvoke-acl-' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $dir | Out-Null
        try {
            Set-RestrictedAcl $dir
            $acl = Get-Acl $dir
            $acl.AreAccessRulesProtected | Should -BeTrue
            $sids = @($acl.Access | ForEach-Object { $_.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value } | Sort-Object -Unique)
            $sids | Should -Be @('S-1-5-18', 'S-1-5-32-544')
            @($acl.Access | Where-Object { $_.FileSystemRights -ne 'FullControl' }).Count | Should -Be 0
        } finally {
            Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}

Describe 'Test-TrustedFolder and Initialize-SecureFolder' -Skip:(-not $onWindows) {
    BeforeAll { . ([scriptblock]::Create((Get-FunctionText $built @('New-RestrictedSecurity', 'Set-RestrictedAcl', 'Test-TrustedFolder', 'Initialize-SecureFolder')))) }

    It 'creates a missing folder with only SYSTEM and Administrators, and accepts it' {
        $dir = Join-Path ([IO.Path]::GetTempPath()) ('nvoke-new-' + [guid]::NewGuid().ToString('N'))
        try {
            Initialize-SecureFolder $dir
            Test-TrustedFolder $dir | Should -BeTrue
            (Get-Acl $dir).AreAccessRulesProtected | Should -BeTrue
        } finally { Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue }
    }

    It 'refuses a junction, which would send SYSTEM somewhere else' {
        $target = Join-Path ([IO.Path]::GetTempPath()) ('nvoke-target-' + [guid]::NewGuid().ToString('N'))
        $link = Join-Path ([IO.Path]::GetTempPath()) ('nvoke-link-' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $target | Out-Null
        New-Item -ItemType Junction -Path $link -Target $target | Out-Null
        try {
            Test-TrustedFolder $link | Should -BeFalse
            { Initialize-SecureFolder $link } | Should -Throw '*link or isn''t owned*'
        } finally {
            [IO.Directory]::Delete($link)
            Remove-Item $target -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}

Describe 'Remove-TreeSafely' {
    BeforeAll {
        . ([scriptblock]::Create((Get-FunctionText $built @('Remove-TreeSafely'))))
        $installer = Join-Path $here 'installAgent.ps1'
        $fromInstaller = Get-FunctionText $installer @('Remove-TreeSafely')
    }

    It 'removes a folder and what is in it' {
        $dir = Join-Path ([IO.Path]::GetTempPath()) ('nvoke-rm-' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path (Join-Path $dir 'a\b') -Force | Out-Null
        Set-Content -Path (Join-Path $dir 'a\b\f.txt') -Value 'x'
        Remove-TreeSafely $dir
        Test-Path $dir | Should -BeFalse
    }

    It 'removes a link inside it without touching what the link points to' -Skip:(-not ($onWindows -or $IsLinux -or $IsMacOS)) {
        $dir = Join-Path ([IO.Path]::GetTempPath()) ('nvoke-rm-' + [guid]::NewGuid().ToString('N'))
        $outside = Join-Path ([IO.Path]::GetTempPath()) ('nvoke-outside-' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $dir, $outside | Out-Null
        Set-Content -Path (Join-Path $outside 'precious.txt') -Value 'keep'
        $type = if ($onWindows) { 'Junction' } else { 'SymbolicLink' }
        New-Item -ItemType $type -Path (Join-Path $dir 'link') -Target $outside | Out-Null
        try {
            Remove-TreeSafely $dir
            Test-Path $dir | Should -BeFalse
            Get-Content (Join-Path $outside 'precious.txt') | Should -Be 'keep'
        } finally { Remove-Item $outside -Recurse -Force -ErrorAction SilentlyContinue }
    }

    It 'is the same function in the installer, which does its own cleanup' {
        $fromInstaller | Should -Not -BeNullOrEmpty
        $text = Get-Content -Raw (Join-Path $here 'installAgent.ps1')
        $text | Should -Not -Match 'Remove-Item -Path \$folder -Recurse'
        $text | Should -Match 'Remove-TreeSafely \$folder'
        ($fromInstaller -replace '\s+', ' ') | Should -Be ((Get-FunctionText $built @('Remove-TreeSafely')) -replace '\s+', ' ')
    }
}

Describe 'Hide-RegistrationToken' {
    BeforeAll {
        # The installer's own functions, so the test uses the code that ships.
        $installer = Join-Path $here 'installAgent.ps1'
        . ([scriptblock]::Create((Get-FunctionText $installer @('Hide-RegistrationToken', 'Write-InstallLog'))))
        $token = 'abcd-1234-efgh-5678'
        $LogDir = Join-Path ([IO.Path]::GetTempPath()) ('nvoke-log-' + [guid]::NewGuid().ToString('N'))
        $LogPath = Join-Path $LogDir 'install-test.log'
        New-Item -ItemType Directory -Path $LogDir | Out-Null
    }
    AfterAll { Remove-Item $LogDir -Recurse -Force -ErrorAction SilentlyContinue }
    BeforeEach { Get-ChildItem $LogDir -Force | Remove-Item -Force }

    It 'removes the token from an ANSI log and leaves every other byte alone' {
        $latin = [Text.Encoding]::GetEncoding(28591)
        $original = "Command Line: REGISTRATION_TOKEN=$token SERVERADDRESS=new.example.com`r`nProperty(S): REGISTRATION_TOKEN = $token`r`nCaf$([char]0xE9) caf$([char]0xE9)"
        $path = Join-Path $LogDir 'msi-1.log'
        [IO.File]::WriteAllBytes($path, $latin.GetBytes($original))
        Hide-RegistrationToken $LogDir $token | Should -Be 1
        $after = $latin.GetString([IO.File]::ReadAllBytes($path))
        $after | Should -Not -Match [regex]::Escape($token)
        $after | Should -Be $original.Replace($token, '[token removed]')
    }

    It 'removes the token from a UTF-16 log and keeps its byte-order mark' {
        $path = Join-Path $LogDir 'msi-2.log'
        $bytes = [Text.Encoding]::Unicode.GetPreamble() + [Text.Encoding]::Unicode.GetBytes("REGISTRATION_TOKEN=$token end")
        [IO.File]::WriteAllBytes($path, $bytes)
        Hide-RegistrationToken $LogDir $token | Should -Be 1
        $after = [IO.File]::ReadAllBytes($path)
        $after[0] | Should -Be 0xFF
        $after[1] | Should -Be 0xFE
        [Text.Encoding]::Unicode.GetString($after) | Should -Not -Match [regex]::Escape($token)
        [Text.Encoding]::Unicode.GetString($after) | Should -Match 'REGISTRATION_TOKEN=\[token removed\] end'
    }

    It 'handles every log in the folder and counts only the ones it changed' {
        [IO.File]::WriteAllText((Join-Path $LogDir 'msi-a.log'), "x $token y $token")
        [IO.File]::WriteAllText((Join-Path $LogDir 'install-b.log'), "token $token")
        [IO.File]::WriteAllText((Join-Path $LogDir 'uninstall-c.log'), 'nothing secret here')
        Hide-RegistrationToken $LogDir $token | Should -Be 2
        (Get-ChildItem $LogDir -Filter '*.log' | Get-Content -Raw) -join '' | Should -Not -Match [regex]::Escape($token)
        Get-ChildItem $LogDir -Filter '*.tmp' | Should -BeNullOrEmpty
    }

    It 'does nothing without a token or a folder' {
        [IO.File]::WriteAllText((Join-Path $LogDir 'msi-a.log'), 'keep')
        Hide-RegistrationToken $LogDir '' | Should -Be 0
        Hide-RegistrationToken $LogDir $null | Should -Be 0
        Hide-RegistrationToken (Join-Path $LogDir 'nope') $token | Should -Be 0
        Get-Content (Join-Path $LogDir 'msi-a.log') -Raw | Should -Be 'keep'
    }
}

Describe 'Get-UfaSyncState' {
    BeforeAll {
        . ([scriptblock]::Create((Get-FunctionText (Join-Path $here 'installAgent.ps1') @('Get-UfaSyncState', 'Test-UfaRepairDue', 'Test-UfaBlockedRestartDue', 'Test-UfaRedeployRestartDue', 'Test-UfaRegistrationBroken', 'Test-UfaBrokenConfirmed', 'Get-UfaRemovalSummary', 'Get-UfaFolderActivity'))))
        $auth = 'https://auth.example.invalid/oauth2/token'
        $timeout = "context deadline exceeded (Client.Timeout exceeded while awaiting headers)"
        $lines = @{
            Fail    = "[2026-10-20T10:27:59.400-04:00] ERROR (212): configservice::FetchAllConfigurations failed to fetch configuration data: Post `"$auth`": $timeout"
            JwtFail = "[2026-10-20T10:27:59.400-04:00] ERROR (212): auth::requestOauthToken unable to get jwt: Post `"$auth`": $timeout"
            AuthOk  = '[2026-10-20T10:53:46.634-04:00] INFO (249): auth::requestOauthToken Oauth token response received. Status: 200.'
            Synced  = "[2026-10-20T11:28:09.180-04:00] INFO (21): components::Download Successfully downloaded 'cdn.example.invalid/components/generic-asset-interrogator/windows/v0.18.2/x.exe' to 'C:\x'"
            Meta    = "[2026-10-20T10:28:26.000-04:00] ERROR (21): localdiscovery::CollectFingerPrintData failed to collect device cloud discovery data: Get `"http://169.254.169.254/metadata/instance`": $timeout"
            Fail400 = '[2026-10-20T10:21:50.445-04:00] ERROR (7): configservice::FetchAllConfigurations failed to fetch configuration data: unexpected status code: 400 body: <html>\r\n<head><title>400 The SSL certificate error</title></head>'
            Ssl400  = '[2026-10-20T10:21:42.239-04:00] ERROR (7): auth::fetchJWTWithRetries error occurred when fetching jwt: unexpected status code: 400 body: <html>\r\n<head><title>400 The SSL certificate error</title></head>'
            Deny403 = "[2026-10-20T13:48:24.328-04:00] ERROR (47): components::doRequestWithRetry received invalid status: '403'"
            Down200 = "[2026-10-20T13:42:58.000-04:00] INFO (15): components::Download Successfully downloaded 'cdn.example.invalid/components/msp-agent-core/windows/v3.43.0/x.exe' to 'C:\x'"
            Noise   = '[2026-10-20T10:30:00.000-04:00] INFO (21): scheduler::Run (configGUID=Check For Updates) Job completed: Check For Updates'
        }
        function Get-State($Lines, [string]$SinceUtc = '2026-10-20T14:00:00Z') {
            $log = Join-Path ([IO.Path]::GetTempPath()) ('nvoke-core-' + [guid]::NewGuid().ToString('N') + '.log')
            try {
                Set-Content -Path $log -Value $Lines -Encoding UTF8
                Get-UfaSyncState $log ([datetime]::Parse($SinceUtc, [Globalization.CultureInfo]::InvariantCulture, [Globalization.DateTimeStyles]::AdjustToUniversal))
            } finally { Remove-Item $log -Force -ErrorAction SilentlyContinue }
        }
    }

    It 'says stalled when a fetch timed out and the cloud answers again with nothing delivered' {
        (Get-State @($lines.JwtFail, $lines.Fail, $lines.Noise, $lines.AuthOk)).State | Should -Be 'stalled'
    }
    It 'says blocked while the cloud still does not answer, so no restart is asked for' {
        (Get-State @($lines.Fail, $lines.Noise)).State | Should -Be 'blocked'
    }
    It 'says rejected when the cloud answers the login with 400 SSL certificate error and never accepts it, so no restart is asked for' {
        (Get-State @($lines.Ssl400, $lines.Fail400, $lines.JwtFail, $lines.Fail)).State | Should -Be 'rejected'
    }
    It 'says stalled, not rejected, when the login is accepted later and nothing was delivered' {
        (Get-State @($lines.Ssl400, $lines.Fail400, $lines.AuthOk)).State | Should -Be 'stalled'
    }
    It 'says denied after repeated CDN 403s with nothing downloaded since, and reports when they began' {
        $r = Get-State @($lines.AuthOk, $lines.Deny403, $lines.Deny403, $lines.Deny403, $lines.Deny403, $lines.Deny403)
        $r.State | Should -Be 'denied'
        $r.FaultSince | Should -Not -BeNullOrEmpty
    }
    It 'does not say denied for a few 403s, or when a download succeeded after them' {
        (Get-State @($lines.Deny403, $lines.Deny403)).State | Should -Be 'waiting'
        (Get-State @($lines.Deny403, $lines.Deny403, $lines.Deny403, $lines.Deny403, $lines.Deny403, $lines.Down200)).State | Should -Be 'waiting'
    }
    It 'says synced once the inventory component was downloaded after the failure' {
        (Get-State @($lines.Fail, $lines.AuthOk, $lines.Synced)).State | Should -Be 'synced'
    }
    It 'says synced when the component arrived and nothing failed' {
        (Get-State @($lines.Noise, $lines.Synced)).State | Should -Be 'synced'
    }
    It 'says waiting when there is no fault and no component yet' {
        (Get-State @($lines.Noise)).State | Should -Be 'waiting'
    }
    It 'does not treat the cloud-metadata timeout, which every non-cloud machine logs, as a fault' {
        (Get-State @($lines.Meta, $lines.Noise, $lines.AuthOk)).State | Should -Be 'waiting'
    }
    It 'ignores everything logged before the install' {
        (Get-State @($lines.Fail, $lines.AuthOk) '2026-10-20T15:00:00Z').State | Should -Be 'waiting'
    }
    It 'reads the log time zone: 10:27:59 at -04:00 is 14:27:59 UTC' {
        (Get-State @($lines.Fail, $lines.AuthOk)).FailedAt.ToString('HH:mm:ss') | Should -Be '14:27:59'
    }
    It 'reports when the refusal began, and forgets it once a login is accepted' {
        (Get-State @($lines.Ssl400, $lines.Fail400)).RejectedSince.ToString('HH:mm:ss') | Should -Be '14:21:42'
        (Get-State @($lines.Ssl400, $lines.Fail400, $lines.AuthOk)).RejectedSince | Should -BeNullOrEmpty
    }
    It 'asks for the repair only after a sustained refusal, and only once' {
        $sync = [pscustomobject]@{ State = 'rejected'; RejectedSince = [datetime]'2026-10-20T14:21:42Z'; FaultSince = [datetime]'2026-10-20T14:21:42Z'; FaultLastAt = [datetime]'2026-10-20T14:50:00Z' }
        Test-UfaRepairDue $sync ([datetime]'2026-10-20T14:40:00Z') $false 30 | Should -BeFalse
        Test-UfaRepairDue $sync ([datetime]'2026-10-20T14:52:00Z') $false 30 | Should -BeTrue
        Test-UfaRepairDue $sync ([datetime]'2026-10-20T14:52:00Z') $true 30 | Should -BeFalse
    }
    It 'asks for the repair for a denied state too, after the same wait' {
        $sync = [pscustomobject]@{ State = 'denied'; FaultSince = [datetime]'2026-10-20T14:00:00Z'; FaultLastAt = [datetime]'2026-10-20T14:30:00Z' }
        Test-UfaRepairDue $sync ([datetime]'2026-10-20T14:20:00Z') $false 30 | Should -BeFalse
        Test-UfaRepairDue $sync ([datetime]'2026-10-20T14:31:00Z') $false 30 | Should -BeTrue
    }
    It 'does not repair when the refusal is old, for example when a network outage followed it' {
        $sync = [pscustomobject]@{ State = 'rejected'; FaultSince = [datetime]'2026-10-20T14:00:00Z'; FaultLastAt = [datetime]'2026-10-20T14:10:00Z' }
        Test-UfaRepairDue $sync ([datetime]'2026-10-20T15:00:00Z') $false 30 45 | Should -BeFalse
        Test-UfaRepairDue $sync ([datetime]'2026-10-20T14:50:00Z') $false 30 45 | Should -BeTrue
    }
    It 'reports when the last fault was logged, only while a fault is the state' {
        (Get-State @($lines.Ssl400, $lines.Fail400, $lines.Fail)).FaultLastAt.ToString('HH:mm:ss') | Should -Be '14:21:42'
        (Get-State @($lines.Fail, $lines.AuthOk, $lines.Synced)).FaultLastAt | Should -BeNullOrEmpty
        (Get-State @($lines.Noise)).FaultLastAt | Should -BeNullOrEmpty
    }
    It 'reports when a blocked state began, and forgets it once a login is accepted' {
        $r = Get-State @($lines.JwtFail, $lines.Fail, $lines.Noise)
        $r.State | Should -Be 'blocked'
        $r.FaultSince.ToString('HH:mm:ss') | Should -Be '14:27:59'
        (Get-State @($lines.Fail, $lines.AuthOk, $lines.Synced)).FaultSince | Should -BeNullOrEmpty
    }
    It 'treats a failed login, not only a failed configuration fetch, as the latest sign of a blocked state' {
        $later = "[2026-10-20T10:57:59.400-04:00] ERROR (55): auth::fetchJWTWithRetries error occurred when fetching jwt: Post `"$auth`": $timeout"
        $r = Get-State @($lines.Fail, $later)
        $r.State | Should -Be 'blocked'
        $r.FailedAt.ToString('HH:mm:ss') | Should -Be '14:27:59'
        $r.FaultLastAt.ToString('HH:mm:ss') | Should -Be '14:57:59'
    }
    It 'restarts MspAgent once for a blocked state that has lasted, only while the failures are current' {
        $sync = [pscustomobject]@{ State = 'blocked'; FaultSince = [datetime]'2026-10-20T14:00:00Z'; FaultLastAt = [datetime]'2026-10-20T14:40:00Z' }
        Test-UfaBlockedRestartDue $sync 0 ([datetime]'2026-10-20T14:25:00Z') 30 | Should -BeFalse
        Test-UfaBlockedRestartDue $sync 0 ([datetime]'2026-10-20T14:31:00Z') 30 | Should -BeTrue
        Test-UfaBlockedRestartDue $sync 1 ([datetime]'2026-10-20T14:45:00Z') 30 | Should -BeFalse
        Test-UfaBlockedRestartDue $sync 0 ([datetime]'2026-10-20T15:30:00Z') 30 | Should -BeFalse
        Test-UfaBlockedRestartDue ([pscustomobject]@{ State = 'stalled'; FaultSince = $sync.FaultSince; FaultLastAt = $sync.FaultLastAt }) 0 ([datetime]'2026-10-20T14:45:00Z') 30 | Should -BeFalse
    }
    It 'repairs a long blocked state only after the restart had time to work and a failure was logged after it' {
        $sync = [pscustomobject]@{ State = 'blocked'; FaultSince = [datetime]'2026-10-20T14:00:00Z'; FaultLastAt = [datetime]'2026-10-20T15:05:00Z' }
        $restart = [datetime]'2026-10-20T14:30:00Z'
        # 60 minutes blocked, one restart 35 minutes ago, a failure since: repair
        Test-UfaRepairDue $sync ([datetime]'2026-10-20T15:05:00Z') $false 30 45 60 1 $restart 20 | Should -BeTrue
        # blocked for less than 60 minutes
        Test-UfaRepairDue $sync ([datetime]'2026-10-20T14:55:00Z') $false 30 45 60 1 $restart 20 | Should -BeFalse
        # no restart tried
        Test-UfaRepairDue $sync ([datetime]'2026-10-20T15:05:00Z') $false 30 45 60 0 ([datetime]::MinValue) 20 | Should -BeFalse
        # the restart is too recent to judge
        Test-UfaRepairDue $sync ([datetime]'2026-10-20T15:05:00Z') $false 30 45 60 1 ([datetime]'2026-10-20T14:55:00Z') 20 | Should -BeFalse
        # no failure logged after the restart
        $before = [pscustomobject]@{ State = 'blocked'; FaultSince = [datetime]'2026-10-20T14:00:00Z'; FaultLastAt = [datetime]'2026-10-20T14:25:00Z' }
        Test-UfaRepairDue $before ([datetime]'2026-10-20T15:00:00Z') $false 30 45 60 1 $restart 20 | Should -BeFalse
        # failures are old
        Test-UfaRepairDue $sync ([datetime]'2026-10-20T16:00:00Z') $false 30 45 60 1 $restart 20 | Should -BeFalse
        # already repaired
        Test-UfaRepairDue $sync ([datetime]'2026-10-20T15:05:00Z') $true 30 45 60 1 $restart 20 | Should -BeFalse
        # without the blocked settings (the default) a blocked state is never repaired
        Test-UfaRepairDue $sync ([datetime]'2026-10-20T15:05:00Z') $false 30 | Should -BeFalse
    }
    It 'never asks for the repair for any other state' {
        foreach ($state in 'stalled', 'waiting', 'synced', 'unknown') {
            $sync = [pscustomobject]@{ State = $state; RejectedSince = [datetime]'2026-10-20T10:00:00Z'; FaultSince = [datetime]'2026-10-20T10:00:00Z'; FaultLastAt = [datetime]'2026-10-20T15:59:00Z' }
            Test-UfaRepairDue $sync ([datetime]'2026-10-20T16:00:00Z') $false 30 45 60 1 ([datetime]'2026-10-20T14:00:00Z') 20 | Should -BeFalse
        }
    }
    It 'restarts the agent services again when the UFA is still missing some minutes after the last restart, a limited number of times' {
        $now = [datetime]'2026-10-20T20:40:00Z'
        Test-UfaRedeployRestartDue $false ([datetime]'2026-10-20T20:35:00Z') 0 $now 10 2 | Should -BeFalse
        Test-UfaRedeployRestartDue $false ([datetime]'2026-10-20T20:28:00Z') 0 $now 10 2 | Should -BeTrue
        Test-UfaRedeployRestartDue $false ([datetime]'2026-10-20T20:00:00Z') 2 $now 10 2 | Should -BeFalse
        Test-UfaRedeployRestartDue $true ([datetime]'2026-10-20T20:00:00Z') 0 $now 10 2 | Should -BeFalse
    }
    It 'does not restart again while the Msp Agent folder is still getting new data, but does once it has gone quiet' {
        $now = [datetime]'2026-10-20T20:40:00Z'
        $last = [datetime]'2026-10-20T20:20:00Z'
        Test-UfaRedeployRestartDue $false $last 0 $now 10 2 ([datetime]'2026-10-20T20:35:00Z') | Should -BeFalse
        Test-UfaRedeployRestartDue $false $last 0 $now 10 2 ([datetime]'2026-10-20T20:25:00Z') | Should -BeTrue
        Test-UfaRedeployRestartDue $false $last 0 $now 10 2 $null | Should -BeTrue
    }
    It 'reports what was written to the Msp Agent folder since a time, ignoring the old leftover executable' {
        $dir = Join-Path ([IO.Path]::GetTempPath()) ('nvoke-msp-' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path (Join-Path $dir 'downloads') -Force | Out-Null
        $utc = { param([string]$s) [datetime]::Parse($s, [Globalization.CultureInfo]::InvariantCulture, [Globalization.DateTimeStyles]::AdjustToUniversal) }
        try {
            $exe = Join-Path $dir 'msp-agent-core.exe'; $new1 = Join-Path $dir 'config.json'; $new2 = Join-Path $dir 'downloads\component.exe'
            foreach ($f in $exe, $new1, $new2) { [IO.File]::WriteAllText($f, 'x') }
            (Get-Item $exe).LastWriteTimeUtc = & $utc '2026-10-21T14:46:00Z'
            (Get-Item $new1).LastWriteTimeUtc = & $utc '2026-10-21T18:10:00Z'
            (Get-Item $new2).LastWriteTimeUtc = & $utc '2026-10-21T18:12:00Z'
            $a = Get-UfaFolderActivity $dir (& $utc '2026-10-21T18:00:00Z')
            $a.Files | Should -Be 2
            $a.NewestAt.ToString('HH:mm') | Should -Be '18:12'
            (Get-UfaFolderActivity $dir (& $utc '2026-10-21T19:00:00Z')).Files | Should -Be 0
            (Get-UfaFolderActivity $dir (& $utc '2026-10-21T19:00:00Z')).NewestAt | Should -BeNullOrEmpty
        } finally { Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue }
        (Get-UfaFolderActivity (Join-Path ([IO.Path]::GetTempPath()) 'nvoke-no-such-folder') ([datetime]::MinValue)).Files | Should -Be 0
    }
    It 'calls an Ecoverse agent broken only when it is registered, its program is gone and it has no service' {
        Test-UfaRegistrationBroken $true $false $false | Should -BeTrue
        Test-UfaRegistrationBroken $false $false $false | Should -BeFalse   # nothing registered: the Windows Agent installs a fresh one
        Test-UfaRegistrationBroken $true $true $false | Should -BeFalse     # the program is there: a removal can run
        Test-UfaRegistrationBroken $true $false $true | Should -BeFalse     # a service exists: it is running or starting
        Test-UfaRegistrationBroken $true $true $true | Should -BeFalse
    }
    It 'reports a broken registration only once it has been seen on two runs some minutes apart' {
        $first = [datetime]'2026-10-21T21:43:00Z'
        Test-UfaBrokenConfirmed $true $first ([datetime]'2026-10-21T21:43:00Z') 10 | Should -BeFalse
        Test-UfaBrokenConfirmed $true $first ([datetime]'2026-10-21T21:50:00Z') 10 | Should -BeFalse
        Test-UfaBrokenConfirmed $true $first ([datetime]'2026-10-21T21:58:00Z') 10 | Should -BeTrue
        Test-UfaBrokenConfirmed $false $first ([datetime]'2026-10-21T23:00:00Z') 10 | Should -BeFalse
        Test-UfaBrokenConfirmed $true $null ([datetime]'2026-10-21T23:00:00Z') 10 | Should -BeFalse
    }
    It 'summarises what Windows Installer logged for the product: failures, the last MSI error and completed removals' {
        $ev = { param($id, $t) [pscustomobject]@{ Id = $id; TimeCreated = [datetime]$t } }
        # SUP-2190, 2026-10-21 (local times): the removal was cut short (1722), then failed with 1721 every time
        $events = @(
            (& $ev 11722 '2026-10-21T17:26:04'), (& $ev 11725 '2026-10-21T17:26:04'),
            (& $ev 11721 '2026-10-21T17:29:04'), (& $ev 11725 '2026-10-21T17:29:04'),
            (& $ev 11721 '2026-10-21T18:13:31'), (& $ev 11725 '2026-10-21T18:13:31')
        )
        $s = Get-UfaRemovalSummary $events
        ($s.Failures, $s.LastErrorCode, $s.Completed) | Should -Be 3, 1721, 0
        $s.LastFailureAt | Should -Not -BeNullOrEmpty
        $none = Get-UfaRemovalSummary @()
        ($none.Failures, $none.Completed) | Should -Be 0, 0
        $none.LastErrorCode | Should -BeNullOrEmpty
        (Get-UfaRemovalSummary @((& $ev 11724 '2026-10-21T14:24:36'))).Completed | Should -Be 1
    }
    It 'only reads: the detection never removes, restarts, stops or uninstalls anything' {
        $path = Join-Path $here 'installAgent.ps1'
        $text = Get-FunctionText $path @('Test-UfaRegistrationBroken', 'Test-UfaBrokenConfirmed', 'Get-UfaRemovalSummary', 'Get-UfaRemovalEvent', 'Get-UfaRegistration')
        $text | Should -Not -Match 'Remove-Item|Remove-TreeSafely|Restart-|Stop-|Start-Process|msiexec|Invoke-WithTimeout|Set-ItemProperty|New-Item'
        (Get-Content -Raw $path) | Should -Not -Match 'ufa-absent-restart|Test-UfaAbsentRestartDue'
    }
    It 'says unknown when there is no log' {
        (Get-UfaSyncState (Join-Path ([IO.Path]::GetTempPath()) 'nvoke-no-such-core.log') ([datetime]::MinValue)).State | Should -Be 'unknown'
    }
}

Describe 'the installer' {
    It 'passes the registration token on the installer command line only, never to the task or a log call' {
        $text = Get-Content -Raw (Join-Path $here 'installAgent.ps1')
        # The only place the token is used is building WindowsAgentSetup.exe's arguments.
        ([regex]::Matches($text, 'registrationToken')).Count | Should -Be 3
        $text | Should -Not -Match 'Write-InstallLog[^\r\n]*registrationToken'
        $text | Should -Not -Match 'Add-StatusHistory[^\r\n]*registrationToken'
    }

    It 'scrubs the installer log straight after the install returns' {
        $text = Get-Content -Raw (Join-Path $here 'installAgent.ps1')
        $text | Should -Match '(?s)Start-Process -FilePath \$SetupPath.*?\r?\n\s*if \(\(Hide-RegistrationToken'
    }

    It 'starts the full cleanup with the UFA core uninstaller, before any product is uninstalled' {
        . ([scriptblock]::Create((Get-FunctionText (Join-Path $here 'installAgent.ps1') @('Invoke-UfaCoreUninstall', 'Invoke-FullCleanup')))) 2>$null
        $text = Get-Content -Raw (Join-Path $here 'installAgent.ps1')
        $cleanup = [regex]::Match($text, '(?s)function Invoke-FullCleanup\(\$Status\) \{.*?foreach \(\$product in \$CleanupProducts\)').Value
        $cleanup | Should -Match 'Invoke-UfaCoreUninstall'
        $core = [regex]::Match($text, '(?s)function Invoke-UfaCoreUninstall \{.*?\r?\n\}').Value
        $core | Should -Match 'Invoke-WithTimeout \$core ''--uninstall'''
    }

    It 'runs the repair in order: uninstall, wait 120 seconds, restart Windows Agent Maintenance Service, 5 seconds, then Windows Agent Service' {
        $text = Get-Content -Raw (Join-Path $here 'installAgent.ps1')
        $repair = [regex]::Match($text, '(?s)function Invoke-UfaRepair\(.*?\r?\n\}').Value
        $repair | Should -Match '\[int\]\$SettleSeconds = 120'
        $repair | Should -Match '(?s)Invoke-UfaCoreUninstall.*Start-Sleep -Seconds \$SettleSeconds.*Restart-WindowsAgentService'
        $restart = [regex]::Match($text, '(?s)function Restart-WindowsAgentService \{.*?\r?\n\}').Value
        $restart | Should -Match "(?s)'Windows Agent Maintenance Service', 'Windows Agent Service'"
        $restart | Should -Match 'Start-Sleep -Seconds 5'
        . ([scriptblock]::Create((Get-FunctionText (Join-Path $here 'installAgent.ps1') @('Invoke-UfaRepair'))))
        $script:calls = @()
        function Invoke-UfaCoreUninstall { $script:calls += 'uninstall'; 'exit code 0' }
        function Restart-WindowsAgentService { $script:calls += 'restart services' }
        $script:UfaRepairRestartedAt = $null
        (Invoke-UfaRepair -SettleSeconds 0) | Should -Match 'uninstall: exit code 0'
        $script:calls | Should -Be 'uninstall', 'restart services'
        $script:UfaRepairRestartedAt | Should -Not -BeNullOrEmpty
    }

    It 'never deletes the leftover Msp Agent folder or msp-agent-core.exe in the repair, because the Windows Agent needs them to redeploy the UFA' {
        $text = Get-Content -Raw (Join-Path $here 'installAgent.ps1')
        $repair = [regex]::Match($text, '(?s)function Invoke-UfaRepair\(.*?\r?\n\}').Value
        $repair | Should -Not -Match 'Remove-Item|Remove-TreeSafely|Invoke-FullCleanup|Test-Path'
    }

    It 'cleans only the legacy N-central agent: never the Ecoverse agent folders, and never its MSI, which only msp-agent-core.exe --uninstall removes' {
        $text = Get-Content -Raw (Join-Path $here 'installAgent.ps1')
        $folders = [regex]::Match($text, '(?s)\$CleanupFolders = @\((.*?)\r?\n\)').Groups[1].Value
        $folders | Should -Match 'N-able Technologies'
        $folders | Should -Match 'MspPlatform'
        $folders | Should -Not -Match 'Msp Agent|MspAgent|MSPScriptExecution'
        $products = [regex]::Match($text, '(?m)^\$CleanupProducts = (.*)$').Groups[1].Value
        $products | Should -Match "'Windows Agent'"
        $products | Should -Not -Match 'MSP Core Agent'
        $text | Should -Match 'cleanup-ufa-leftover'
    }

    It 'waits for the Ecoverse agent''s own MSI removal to finish before it stops any process or deletes any folder, and never removes that product itself' {
        $cleanup = Get-FunctionText (Join-Path $here 'installAgent.ps1') @('Invoke-FullCleanup')
        $wait = $cleanup.IndexOf('cleanup-ufa-wait')
        $wait | Should -BeGreaterThan 0
        $wait | Should -BeLessThan $cleanup.IndexOf('Stop-Process')
        $wait | Should -BeLessThan $cleanup.IndexOf('Remove-TreeSafely')
        $cleanup | Should -Match "Get-UninstallEntry 'MSP Core Agent'"
        $cleanup | Should -Not -Match 'msiexec.*MSP Core Agent|Invoke-WithTimeout[^\r\n]*MSP Core Agent'
    }

    It 'is ASCII' {
        ($bytes | Where-Object { $_ -gt 126 -or ($_ -lt 32 -and $_ -notin 9, 10, 13) }).Count | Should -Be 0
    }
}

Describe 'the lab log summary probe (ufa-log-summary.ps1)' {
    BeforeAll {
        $probe = [IO.Path]::GetFullPath((Join-Path $here '../lab/probes/ufa-log-summary.ps1'))
        $timeout = 'context deadline exceeded (Client.Timeout exceeded while awaiting headers)'
        $blockedLog = @(
            '[2026-10-21T10:50:35.000-04:00] ERROR (19): configservice::FetchAllConfigurations failed to fetch configuration data: Post "https://auth.example.invalid/oauth2/token": ' + $timeout
            '[2026-10-21T11:00:10.000-04:00] ERROR (184): auth::requestOauthToken unable to get jwt: Post "https://auth.example.invalid/oauth2/token": ' + $timeout
            '[2026-10-21T11:00:10.001-04:00] ERROR (184): auth::fetchJWTWithRetries error occurred when fetching jwt: Post "https://auth.example.invalid/oauth2/token": ' + $timeout
        )
        function Invoke-Probe([string[]]$Lines) {
            $log = Join-Path ([IO.Path]::GetTempPath()) ('nvoke-probe-' + [guid]::NewGuid().ToString('N') + '.log')
            try {
                Set-Content -Path $log -Value $Lines -Encoding UTF8
                (& $hostPath -NoProfile -ExecutionPolicy Bypass -File $probe -LogPath $log 2>&1 | Out-String)
            } finally { Remove-Item $log -Force -ErrorAction SilentlyContinue }
        }
    }

    It 'uses the installer''s own Get-UfaSyncState, character for character, so the two cannot disagree about a state' {
        $installer = Get-FunctionText (Join-Path $here 'installAgent.ps1') @('Get-UfaSyncState')
        $copy = Get-FunctionText $probe @('Get-UfaSyncState')
        ($copy -replace "`r`n", "`n") | Should -Be ($installer -replace "`r`n", "`n")
    }

    It 'parses, and says blocked for a log the installer calls blocked, with when it began' {
        $errors = $null
        [System.Management.Automation.Language.Parser]::ParseFile($probe, [ref]$null, [ref]$errors) | Out-Null
        @($errors).Count | Should -Be 0
        $text = Invoke-Probe $blockedLog
        $text | Should -Match 'STATE: blocked \('
        $text | Should -Match 'failing since \d\d:\d\d:\d\d, last failure \d\d:\d\d:\d\d'
        $text | Should -Match 'timeouts: 1'
    }

    It 'says synced once the inventory component arrived after the failure' {
        $synced = "[2026-10-21T12:00:00.000-04:00] INFO (21): components::Download Successfully downloaded 'cdn.example.invalid/components/generic-asset-interrogator/windows/v0.18.2/x.exe' to 'C:\x'"
        (Invoke-Probe ($blockedLog + $synced)) | Should -Match 'STATE: synced \('
    }
}
