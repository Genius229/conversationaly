[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9a-fA-F]{40}$')]
    [string]$SourceCommit,
    [Parameter(Mandatory = $true)]
    [string]$OutputDirectory
)

$ErrorActionPreference = 'Stop'
$version = '1.0.0'
$sourceRoot = $PSScriptRoot
$repoRoot = [IO.Path]::GetFullPath((Join-Path $sourceRoot '..\..'))
$head = (& git -C $repoRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $head -ne $SourceCommit) { throw 'Source commit does not match checkout' }
$dirty = @(& git -C $repoRoot status --porcelain -- tools/windows-capture-diagnostics)
if ($LASTEXITCODE -ne 0 -or $dirty.Count -ne 0) { throw 'Diagnostic source must be committed before packaging' }

$files = @('START_DIAGNOSTICS.cmd', 'collect-dshow.ps1', 'README.txt')
if (Test-Path -LiteralPath (Join-Path $sourceRoot 'CaptureProbe.cs') -PathType Leaf) { $files += 'CaptureProbe.cs' }
$manifestFiles = @()
foreach ($name in $files) {
    $file = Get-Item -LiteralPath (Join-Path $sourceRoot $name) -ErrorAction Stop
    if ($file.PSIsContainer -or ($file.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw "Expected a regular source file: $name" }
    $manifestFiles += [ordered]@{
        name = $name
        bytes = $file.Length
        sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    }
}
$manifest = [ordered]@{
    schemaVersion = 1
    tool = 'Conversationaly Capture Diagnostics'
    version = $version
    sourceCommit = $SourceCommit.ToLowerInvariant()
    requiresInstalledApp = $true
    bundlesFfmpeg = $false
    savesAudio = $false
    uploadsReports = $false
    files = $manifestFiles
}
$utf8 = New-Object Text.UTF8Encoding($false)
$manifestBytes = $utf8.GetBytes(($manifest | ConvertTo-Json -Depth 6) + "`n")
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$zipPath = Join-Path $OutputDirectory "Conversationaly-Capture-Diagnostics-$version.zip"
if (Test-Path -LiteralPath $zipPath) { throw "Refusing to replace an existing kit: $zipPath" }
Add-Type -AssemblyName System.IO.Compression
$stream = [IO.File]::Open($zipPath, [IO.FileMode]::CreateNew)
$zip = New-Object IO.Compression.ZipArchive($stream, [IO.Compression.ZipArchiveMode]::Create, $false)
try {
    foreach ($name in ($files + 'MANIFEST.json')) {
        $entry = $zip.CreateEntry("Conversationaly-Capture-Diagnostics/$name", [IO.Compression.CompressionLevel]::Optimal)
        $entry.LastWriteTime = [DateTimeOffset]::Parse('2000-01-01T00:00:00+00:00')
        $target = $entry.Open()
        try {
            if ($name -eq 'MANIFEST.json') { $bytes = $manifestBytes }
            else { $bytes = [IO.File]::ReadAllBytes((Join-Path $sourceRoot $name)) }
            $target.Write($bytes, 0, $bytes.Length)
        } finally { $target.Dispose() }
    }
} finally { $zip.Dispose(); $stream.Dispose() }
$hash = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
[IO.File]::WriteAllText((Join-Path $OutputDirectory 'SHA256SUMS'), "$hash  $([IO.Path]::GetFileName($zipPath))`n", $utf8)
Write-Host "Packaged $zipPath"
Write-Host "SHA256 $hash"
