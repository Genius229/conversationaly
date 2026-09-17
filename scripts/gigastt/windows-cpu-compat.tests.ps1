[CmdletBinding()]
param([string]$ReportPath = '')

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$results = [Collections.Generic.List[object]]::new()
$helper = Join-Path $PSScriptRoot 'windows-cpu-compat.ps1'
if (Test-Path -LiteralPath $helper) { . $helper }
function Assert-True($Value, [string]$Message) { if (-not $Value) { throw $Message } }
function Assert-Throws([scriptblock]$Action, [string]$Pattern = '') {
    $threw = $false
    try { & $Action | Out-Null } catch {
        if ($Pattern -and $_.Exception.Message -notmatch $Pattern) { throw "Unexpected rejection: $($_.Exception.Message)" }
        $threw = $true
    }
    Assert-True $threw 'Expected rejection'
}
function Test-Case([string]$Name, [scriptblock]$Action) {
    try { & $Action; $results.Add(@{ name = $Name; passed = $true }); Write-Host "PASS $Name" }
    catch { $results.Add(@{ name = $Name; passed = $false; error = $_.Exception.Message }); Write-Host "FAIL $Name : $($_.Exception.Message)" }
}
function Hash-Bytes([byte[]]$Bytes) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($sha.ComputeHash($Bytes))).Replace('-', '').ToLowerInvariant() }
    finally { $sha.Dispose() }
}
Add-Type -AssemblyName System.IO.Compression
$root = Join-Path ([IO.Path]::GetTempPath()) ('gigastt-compat-test-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
function New-Fixture([string]$Name, [string]$Mode = '') {
    $path = Join-Path $root "$Name.zip"
    $prefix = 'onnxruntime-win-x64-1.28.2/'
    $bytes = [Text.Encoding]::UTF8.GetBytes('fake DLL for unit validation only')
    $file = [pscustomobject]@{ source = 'lib/onnxruntime.dll'; destination = 'onnxruntime.dll'; bytes = $bytes.Length; sha256 = (Hash-Bytes $bytes) }
    $entries = @(
        @{ name = 'GIT_COMMIT_ID'; content = ('a' * 40) },
        @{ name = 'VERSION_NUMBER'; content = $(if ($Mode -eq 'metadata') { '1.0.0' } else { '1.28.2' }) },
        @{ name = 'unwanted.pdb'; content = 'not shipped' }
    )
    if ($Mode -ne 'missing') { $entries += @{ name = $file.source; content = $(if ($Mode -eq 'corrupt') { 'wrong bytes' } else { [Text.Encoding]::UTF8.GetString($bytes) }) } }
    if ($Mode -eq 'duplicate') { $entries += @{ name = $file.source; content = [Text.Encoding]::UTF8.GetString($bytes) } }
    $stream = [IO.File]::Create($path)
    $zip = [IO.Compression.ZipArchive]::new($stream, [IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($entry in $entries) {
            $target = $zip.CreateEntry($prefix + $entry.name).Open()
            try { $data = [Text.Encoding]::UTF8.GetBytes($entry.content); $target.Write($data, 0, $data.Length) }
            finally { $target.Dispose() }
        }
    } finally { $zip.Dispose(); $stream.Dispose() }
    return [pscustomobject]@{
        path = $path
        profile = [pscustomobject]@{ ort = [pscustomobject]@{
            version = '1.28.2'; sourceCommit = ('a' * 40); archiveRoot = $prefix
            archiveBytes = (Get-Item -LiteralPath $path).Length
            archiveSha256 = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
            files = @($file)
        } }
    }
}
try {
    Test-Case 'production pins and dynamic profile are explicit' {
        $p = Read-CpuCompatibilityProfile -Path (Join-Path $PSScriptRoot 'windows-cpu-compat.json')
        Assert-True ($p.profile -eq 'cpu-compat-v1') 'profile'
        Assert-True ($p.cpuBaseline -eq 'x86-64') 'baseline'
        Assert-True ($p.ort.version -eq '1.28.2' -and $p.ort.apiVersion -eq 27) 'ORT/API pin'
        Assert-True ($p.ort.archiveSha256 -eq 'c4eedd29489d5feca21866d054638416f3655bf6b18851b3b6b85c8313e95c35') 'archive pin'
        Assert-True (@($p.ort.files).Count -eq 4) 'runtime and redistribution notices'
    }
    Test-Case 'legacy arguments unchanged and compatible arguments disable diarization' {
        $legacy = @('build','--release','--locked','--target','x86_64-pc-windows-msvc','-p','gigastt')
        $a = @(Get-CpuProfileBuildArguments -LegacyArguments $legacy)
        Assert-True (($a -join '|') -ceq ($legacy -join '|')) 'legacy changed'
        $p = Read-CpuCompatibilityProfile -Path (Join-Path $PSScriptRoot 'windows-cpu-compat.json')
        $a = @(Get-CpuProfileBuildArguments -LegacyArguments $legacy -Profile $p)
        Assert-True (($a -join '|') -ceq (($legacy + @('--no-default-features','--features','gigastt-core/ort-load-dynamic')) -join '|')) 'compat args'
    }
    Test-Case 'unknown production profile rejected' {
        $source = Get-Content (Join-Path $PSScriptRoot 'windows-cpu-compat.json') -Raw
        $path = Join-Path $root 'bad-profile.json'
        $source.Replace('cpu-compat-v1','unknown') | Set-Content -LiteralPath $path
        Assert-Throws { Read-CpuCompatibilityProfile -Path $path } 'Unknown CPU compatibility profile'
    }
    Test-Case 'profile resolves isolated binary and build script exposes opt-in only' {
        $legacyPath = 'target/x86_64-pc-windows-msvc/release/gigastt.exe'
        Assert-True ((Get-CpuProfileBinaryPath -SourceDir $root -LegacyRelativePath $legacyPath) -eq (Join-Path $root $legacyPath)) 'legacy binary path'
        Assert-True ((Get-CpuProfileBinaryPath -SourceDir $root -LegacyRelativePath $legacyPath -CpuCompatible) -eq (Join-Path $root 'target-cpu-compat/x86_64-pc-windows-msvc/release/gigastt.exe')) 'compat binary path'
        $errors = $null; $tokens = $null
        $ast = [Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'windows-build.ps1'), [ref]$tokens, [ref]$errors)
        Assert-True ($errors.Count -eq 0) 'build script parse failure'
        $params = @($ast.ParamBlock.Parameters | ForEach-Object { $_.Name.VariablePath.UserPath })
        Assert-True ('CpuCompatible' -in $params -and 'OrtArchivePath' -in $params) 'missing opt-in interface'
    }
    Test-Case 'build environment restored even after exception' {
        $names = @('RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','CARGO_TARGET_DIR','CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS','ORT_DYLIB_PATH')
        $original = @{}
        foreach ($name in $names) { $original[$name] = [Environment]::GetEnvironmentVariable($name); [Environment]::SetEnvironmentVariable($name, 'sentinel') }
        try {
            $script:inside = $false
            Assert-Throws {
                Invoke-CpuCompatibilityEnvironment -SourceDir $root -Action {
                    Assert-True ($env:RUSTFLAGS -eq '-C target-cpu=x86-64') 'wrong Rust floor'
                    Assert-True ([string]::IsNullOrEmpty($env:CARGO_ENCODED_RUSTFLAGS)) 'encoded flags leaked'
                    Assert-True ([string]::IsNullOrEmpty($env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS)) 'target flags leaked'
                    Assert-True ([string]::IsNullOrEmpty($env:ORT_DYLIB_PATH)) 'ambient ORT leaked'
                    Assert-True ($env:CARGO_TARGET_DIR -eq (Join-Path $root 'target-cpu-compat')) 'isolated cache'
                    $script:inside = $true
                    throw 'simulated cargo failure'
                }
            }
            Assert-True $script:inside 'action not reached'
            foreach ($name in $names) { Assert-True ([Environment]::GetEnvironmentVariable($name) -eq 'sentinel') "not restored: $name" }
        } finally { foreach ($name in $names) { [Environment]::SetEnvironmentVariable($name, $original[$name]) } }
    }
    Test-Case 'valid archive stages only allowlisted bytes' {
        $f = New-Fixture 'valid'; $out = Join-Path $root 'valid-stage'
        $verified = Get-VerifiedCpuRuntime -ArchivePath $f.path -Profile $f.profile
        Write-VerifiedCpuRuntime -Verified $verified -OutputDir $out
        $files = @(Get-ChildItem -LiteralPath $out -File)
        Assert-True ($files.Count -eq 1 -and $files[0].Name -eq 'onnxruntime.dll') 'unexpected stage files'
        Assert-True ((Get-FileHash $files[0].FullName -Algorithm SHA256).Hash.ToLowerInvariant() -eq $f.profile.ort.files[0].sha256) 'staged hash mismatch'
    }
    Test-Case 'stale native DLLs in compatibility cargo output are rejected' {
        $release = Join-Path $root 'cargo-release'
        New-Item -ItemType Directory -Path $release | Out-Null
        [IO.File]::WriteAllText((Join-Path $release 'gigastt.exe'), 'fixture')
        Assert-CpuCompatibilityReleaseDirectory -Path $release
        [IO.File]::WriteAllText((Join-Path $release 'DirectML.dll'), 'stale cache fixture')
        Assert-Throws { Assert-CpuCompatibilityReleaseDirectory -Path $release } 'Unexpected DLL'
    }
    foreach ($mode in @('corrupt','missing','duplicate','metadata')) {
        Test-Case "reject $mode before writing any runtime" {
            $f = New-Fixture $mode $mode
            Assert-Throws { Get-VerifiedCpuRuntime -ArchivePath $f.path -Profile $f.profile } 'member (length|hash) mismatch|Missing or duplicate required|metadata mismatch'
        }
    }
    Test-Case 'archive hash and unsafe destination rejected' {
        $f = New-Fixture 'hash'; $f.profile.ort.archiveSha256 = ('0' * 64)
        Assert-Throws { Get-VerifiedCpuRuntime -ArchivePath $f.path -Profile $f.profile } 'archive hash mismatch'
        $f = New-Fixture 'traversal'; $f.profile.ort.files[0].destination = '../escape.dll'
        Assert-Throws { Get-VerifiedCpuRuntime -ArchivePath $f.path -Profile $f.profile } 'Unsafe runtime destination'
    }
} finally { Remove-Item -LiteralPath $root -Recurse -Force }
$failed = @($results | Where-Object { -not $_.passed }).Count
$report = [ordered]@{ passed = $results.Count - $failed; failed = $failed; tests = @($results.ToArray()) }
if ($ReportPath) { $report | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $ReportPath -Encoding utf8 }
Write-Host "Tests: $($report.passed) passed, $failed failed"
if ($failed) { exit 1 }
