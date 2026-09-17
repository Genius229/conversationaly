# Pure profile/staging helpers. Dot-sourcing never downloads or launches code.

function Read-CpuCompatibilityProfile {
    [CmdletBinding()]
    param([Parameter(Mandatory = $true)][string]$Path)
    $profile = Get-Content -LiteralPath $Path -Raw -ErrorAction Stop | ConvertFrom-Json
    if ($profile.schemaVersion -ne 1 -or $profile.profile -cne 'cpu-compat-v1') { throw 'Unknown CPU compatibility profile' }
    if ($profile.cpuBaseline -cne 'x86-64' -or $profile.rustFlags -cne '-C target-cpu=x86-64') { throw 'Unexpected Rust CPU floor' }
    if (($profile.cargoArguments -join '|') -cne '--no-default-features|--features|gigastt-core/ort-load-dynamic') { throw 'Unexpected compatibility cargo arguments' }
    if ($profile.ort.apiVersion -ne 27) { throw 'Unexpected ONNX Runtime API contract' }
    if ($profile.ort.version -notmatch '^\d+\.\d+\.\d+$' -or $profile.ort.sourceCommit -notmatch '^[0-9a-f]{40}$') { throw 'Invalid ONNX Runtime version/source pin' }
    if ($profile.ort.archiveSha256 -notmatch '^[0-9a-f]{64}$' -or $profile.ort.archiveBytes -le 0) { throw 'Invalid ONNX Runtime archive pin' }
    $expected = @('onnxruntime.dll','onnxruntime_providers_shared.dll','ONNXRUNTIME-LICENSE.txt','ONNXRUNTIME-ThirdPartyNotices.txt') | Sort-Object
    $actual = @($profile.ort.files | ForEach-Object { $_.destination }) | Sort-Object
    if (($actual -join '|') -cne ($expected -join '|')) { throw 'Unexpected compatibility runtime allowlist' }
    return $profile
}

function Get-CpuProfileBuildArguments {
    [CmdletBinding()]
    param([Parameter(Mandatory = $true)][string[]]$LegacyArguments, $Profile = $null)
    if ($null -eq $Profile) { return $LegacyArguments }
    return [string[]]@($LegacyArguments + $Profile.cargoArguments)
}

function Get-CpuProfileBinaryPath {
    [CmdletBinding()]
    param([Parameter(Mandatory = $true)][string]$SourceDir,
          [Parameter(Mandatory = $true)][string]$LegacyRelativePath,
          [switch]$CpuCompatible)
    if ($CpuCompatible) { return Join-Path $SourceDir 'target-cpu-compat/x86_64-pc-windows-msvc/release/gigastt.exe' }
    return Join-Path $SourceDir $LegacyRelativePath
}

function Invoke-CpuCompatibilityEnvironment {
    [CmdletBinding()]
    param([Parameter(Mandatory = $true)][string]$SourceDir, [Parameter(Mandatory = $true)][scriptblock]$Action)
    $values = @{
        RUSTFLAGS = '-C target-cpu=x86-64'
        CARGO_ENCODED_RUSTFLAGS = $null
        CARGO_TARGET_DIR = (Join-Path ([IO.Path]::GetFullPath($SourceDir)) 'target-cpu-compat')
        CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = $null
        ORT_DYLIB_PATH = $null
    }
    $original = @{}
    foreach ($key in $values.Keys) { $original[$key] = [Environment]::GetEnvironmentVariable($key, 'Process') }
    try {
        foreach ($key in $values.Keys) { [Environment]::SetEnvironmentVariable($key, $values[$key], 'Process') }
        & $Action
    } finally {
        foreach ($key in $values.Keys) { [Environment]::SetEnvironmentVariable($key, $original[$key], 'Process') }
    }
}

function Get-CpuRuntimeBytesHash {
    param([Parameter(Mandatory = $true)][byte[]]$Bytes)
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($sha.ComputeHash($Bytes))).Replace('-', '').ToLowerInvariant() }
    finally { $sha.Dispose() }
}

function Assert-CpuCompatibilityReleaseDirectory {
    [CmdletBinding()]
    param([Parameter(Mandatory = $true)][string]$Path)
    if (@(Get-ChildItem -LiteralPath $Path -File -Filter '*.dll' -ErrorAction Stop).Count -ne 0) {
        throw 'Unexpected DLL in compatibility cargo release directory; use a clean compatibility target cache'
    }
}

function Get-VerifiedCpuRuntime {
    [CmdletBinding()]
    param([Parameter(Mandatory = $true)][string]$ArchivePath, [Parameter(Mandatory = $true)]$Profile)
    $archive = Get-Item -LiteralPath $ArchivePath -ErrorAction Stop
    if ($archive.PSIsContainer -or ($archive.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'ORT archive must be a regular file' }
    if ($archive.Length -ne $Profile.ort.archiveBytes) { throw 'ORT archive length mismatch' }
    if ((Get-FileHash -LiteralPath $archive.FullName -Algorithm SHA256).Hash.ToLowerInvariant() -cne $Profile.ort.archiveSha256) { throw 'ORT archive hash mismatch' }
    $root = [string]$Profile.ort.archiveRoot
    if ($root -notmatch '^onnxruntime-win-x64-[0-9.]+/$') { throw 'Unsafe archive root' }
    $destinations = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($file in $Profile.ort.files) {
        if ($file.destination -notmatch '^[A-Za-z0-9_.-]+$' -or $file.destination -in @('.', '..')) { throw 'Unsafe runtime destination' }
        if (-not $destinations.Add([string]$file.destination)) { throw 'Duplicate runtime destination' }
        if ($file.source -notmatch '^(lib/)?[A-Za-z0-9_.-]+$' -or $file.source -in @('.', '..')) { throw 'Unsafe archive member' }
        if ($file.bytes -le 0 -or $file.bytes -gt 128MB -or $file.sha256 -notmatch '^[0-9a-f]{64}$') { throw 'Invalid member length/hash pin' }
    }
    Add-Type -AssemblyName System.IO.Compression
    $stream = [IO.File]::OpenRead($archive.FullName)
    $zip = [IO.Compression.ZipArchive]::new($stream, [IO.Compression.ZipArchiveMode]::Read)
    $payload = [Collections.Generic.List[object]]::new()
    try {
        $entries = @{}
        $required = @($Profile.ort.files | ForEach-Object { $root + $_.source }) + @(($root + 'GIT_COMMIT_ID'), ($root + 'VERSION_NUMBER'))
        foreach ($name in $required) {
            $matches = @($zip.Entries | Where-Object { $_.FullName -ceq $name })
            if ($matches.Count -ne 1) { throw "Missing or duplicate required archive member: $name" }
            $entries[$name] = $matches[0]
        }
        foreach ($meta in @(@{ name = 'GIT_COMMIT_ID'; expected = $Profile.ort.sourceCommit }, @{ name = 'VERSION_NUMBER'; expected = $Profile.ort.version })) {
            $entry = $entries[$root + $meta.name]
            if ($entry.Length -gt 256) { throw 'ORT metadata length exceeds limit' }
            $reader = [IO.StreamReader]::new($entry.Open(), [Text.UTF8Encoding]::new($false, $true))
            try { $value = $reader.ReadToEnd().Trim() } finally { $reader.Dispose() }
            if ($value -cne $meta.expected) { throw "ORT metadata mismatch: $($meta.name)" }
        }
        foreach ($file in $Profile.ort.files) {
            $entry = $entries[$root + $file.source]
            if ($entry.Length -ne $file.bytes) { throw "ORT member length mismatch: $($file.source)" }
            $inputStream = $entry.Open()
            $memory = [IO.MemoryStream]::new()
            try { $inputStream.CopyTo($memory); $bytes = $memory.ToArray() }
            finally { $memory.Dispose(); $inputStream.Dispose() }
            if ($bytes.Length -ne $file.bytes -or (Get-CpuRuntimeBytesHash $bytes) -cne $file.sha256) { throw "ORT member hash mismatch: $($file.source)" }
            $payload.Add([pscustomobject]@{ name = $file.destination; bytes = $bytes })
        }
    } finally { $zip.Dispose(); $stream.Dispose() }
    # Nothing reaches the output stage until every selected member is verified.
    return [pscustomobject]@{ files = $payload.ToArray(); profile = $Profile }
}

function Write-VerifiedCpuRuntime {
    [CmdletBinding()]
    param([Parameter(Mandatory = $true)]$Verified, [Parameter(Mandatory = $true)][string]$OutputDir)
    New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null
    foreach ($file in $Verified.files) { [IO.File]::WriteAllBytes((Join-Path $OutputDir $file.name), $file.bytes) }
}
