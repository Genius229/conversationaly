[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$SourceDir,
    [Parameter(Mandatory = $true)][string]$OutputDir,
    [string]$ManifestPath = (Join-Path $PSScriptRoot "windows-resources.json")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

function Invoke-NativeChecked {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [Parameter(Mandatory = $true)][string[]]$ArgumentList,
        [Parameter(Mandatory = $true)][string]$Description
    )

    & $FilePath @ArgumentList
    if ($LASTEXITCODE -ne 0) {
        throw "$Description failed with exit code $LASTEXITCODE"
    }
}

function Get-Sha256Lower {
    param([Parameter(Mandatory = $true)][string]$Path)
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

if (-not $IsWindows) {
    throw "windows-build.ps1 must run on native Windows"
}

$manifestFile = (Resolve-Path -LiteralPath $ManifestPath).Path
$manifest = Get-Content -LiteralPath $manifestFile -Raw | ConvertFrom-Json
$source = (Resolve-Path -LiteralPath $SourceDir).Path
$output = [IO.Path]::GetFullPath($OutputDir)

if ((Split-Path -Leaf $output) -ne "gigastt-runtime") {
    throw "Refusing to replace unexpected output directory '$output'; leaf must be gigastt-runtime"
}

$expectedCommit = [string]$manifest.gigastt.sourceCommit
$head = (& git -C $source rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $head -ne $expectedCommit) {
    throw "GigaSTT source pin mismatch: expected $expectedCommit, got '$head'"
}

$tagCommit = (& git -C $source rev-list -n 1 ([string]$manifest.gigastt.sourceTag)).Trim()
if ($LASTEXITCODE -ne 0 -or $tagCommit -ne $expectedCommit) {
    throw "GigaSTT tag pin mismatch: $($manifest.gigastt.sourceTag) resolves to '$tagCommit', expected $expectedCommit"
}

$dirty = @(& git -C $source status --porcelain --untracked-files=no)
if ($LASTEXITCODE -ne 0 -or $dirty.Count -ne 0) {
    throw "Pinned GigaSTT tracked source is not clean before build"
}

$rustVersion = (& rustc --version).Trim()
if ($LASTEXITCODE -ne 0 -or $rustVersion -notmatch "^rustc $([regex]::Escape([string]$manifest.toolchain.rust))\b") {
    throw "Rust pin mismatch: expected $($manifest.toolchain.rust), got '$rustVersion'"
}

$protocVersion = (& protoc --version).Trim()
if ($LASTEXITCODE -ne 0 -or $protocVersion -notmatch "\b$([regex]::Escape([string]$manifest.toolchain.protoc))$") {
    throw "protoc pin mismatch: expected $($manifest.toolchain.protoc), got '$protocVersion'"
}

$cargoArgs = @($manifest.build.cargoArguments | ForEach-Object { [string]$_ })
Write-Host ("Pinned build: cargo {0}" -f ($cargoArgs -join " "))
Push-Location $source
try {
    Invoke-NativeChecked -FilePath "cargo" -ArgumentList $cargoArgs -Description "Pinned GigaSTT Windows build"
}
finally {
    Pop-Location
}

$sourceBinary = Join-Path $source ([string]$manifest.build.binaryRelativePath)
if (-not (Test-Path -LiteralPath $sourceBinary -PathType Leaf)) {
    throw "Build succeeded but binary is missing at $sourceBinary"
}

$versionOutput = (& $sourceBinary --version).Trim()
if ($LASTEXITCODE -ne 0 -or $versionOutput -ne "gigastt $($manifest.gigastt.version)") {
    throw "Built binary version mismatch: got '$versionOutput'"
}

$serveHelp = (& $sourceBinary serve --help) -join "`n"
if ($LASTEXITCODE -ne 0) {
    throw "gigastt serve --help failed with exit code $LASTEXITCODE"
}
foreach ($requiredFlag in @(
    "--host",
    "--port",
    "--model-dir",
    "--model-variant",
    "--punctuation",
    "--punct-model-dir",
    "--itn",
    "--vad",
    "--vad-model-dir",
    "--pool-size",
    "--enable-jobs",
    "--shutdown-drain-secs"
)) {
    if ($serveHelp -notmatch [regex]::Escape($requiredFlag)) {
        throw "Pinned binary serve help is missing required flag $requiredFlag"
    }
}

if (Test-Path -LiteralPath $output) {
    Remove-Item -LiteralPath $output -Recurse -Force
}
New-Item -ItemType Directory -Path $output -Force | Out-Null

$stagedBinary = Join-Path $output ([string]$manifest.packaging.binaryName)
Copy-Item -LiteralPath $sourceBinary -Destination $stagedBinary

# ort's default Windows build is normally self-contained. If this or another
# dependency emits app-local DLLs beside the release binary, stage every one
# beside gigastt.exe so Windows' normal loader search finds it in Tauri's
# resource directory.
$releaseDir = Split-Path -Parent $sourceBinary
Get-ChildItem -LiteralPath $releaseDir -File -Filter "*.dll" | ForEach-Object {
    Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $output $_.Name)
}

$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio/Installer/vswhere.exe"
if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) {
    throw "vswhere.exe is unavailable; cannot produce a fail-closed PE runtime inventory"
}
$dumpbinCandidates = @(& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -find "VC\Tools\MSVC\**\bin\Hostx64\x64\dumpbin.exe")
if ($LASTEXITCODE -ne 0 -or $dumpbinCandidates.Count -eq 0) {
    throw "dumpbin.exe is unavailable; cannot inspect native runtime dependencies"
}
$dumpbin = [string]$dumpbinCandidates[-1]

$initialPeFiles = @(Get-ChildItem -LiteralPath $output -File | Where-Object { $_.Extension -in @(".exe", ".dll") })
if ($initialPeFiles.Count -eq 0) {
    throw "No PE files were staged"
}

$windowsComponentDlls = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
@(
    "advapi32.dll", "bcrypt.dll", "bcryptprimitives.dll", "combase.dll",
    "comdlg32.dll", "crypt32.dll", "dbghelp.dll", "dnsapi.dll", "gdi32.dll",
    "iphlpapi.dll", "kernel32.dll", "kernelbase.dll", "msvcp_win.dll",
    "ncrypt.dll", "netapi32.dll", "normaliz.dll", "ntdll.dll", "ole32.dll",
    "oleaut32.dll", "powrprof.dll", "psapi.dll", "rpcrt4.dll", "secur32.dll",
    "setupapi.dll", "shell32.dll", "shlwapi.dll", "ucrtbase.dll", "user32.dll",
    "userenv.dll", "version.dll", "winhttp.dll", "winmm.dll", "wintrust.dll",
    "ws2_32.dll"
) | ForEach-Object { $windowsComponentDlls.Add($_) | Out-Null }

$visualStudioRoot = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Redist.14.Latest -property installationPath).Trim()
if ($LASTEXITCODE -ne 0) {
    throw "vswhere failed while locating the Visual C++ redistributable"
}

function Find-VcRuntimeDll {
    param([Parameter(Mandatory = $true)][string]$Name)
    if ([string]::IsNullOrWhiteSpace($visualStudioRoot)) {
        return $null
    }
    $redistRoot = Join-Path $visualStudioRoot "VC/Redist/MSVC"
    if (-not (Test-Path -LiteralPath $redistRoot -PathType Container)) {
        return $null
    }
    return @(Get-ChildItem -LiteralPath $redistRoot -Recurse -File -Filter $Name | Where-Object {
        $_.FullName -match "(?i)[\\/]x64[\\/]Microsoft\.VC\d+\.CRT[\\/]"
    } | Sort-Object FullName -Descending | Select-Object -First 1)[0]
}

$imports = [Collections.Generic.List[object]]::new()
$rawInventory = [Collections.Generic.List[string]]::new()
$pendingPeFiles = [Collections.Generic.Queue[string]]::new()
foreach ($pe in $initialPeFiles) {
    $pendingPeFiles.Enqueue($pe.FullName)
}
$inspectedPeFiles = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)

while ($pendingPeFiles.Count -gt 0) {
    $pePath = $pendingPeFiles.Dequeue()
    if (-not $inspectedPeFiles.Add($pePath)) {
        continue
    }
    $pe = Get-Item -LiteralPath $pePath
    $headers = (& $dumpbin /nologo /headers $pe.FullName 2>&1) -join "`n"
    if ($LASTEXITCODE -ne 0 -or $headers -notmatch "(?im)^\s*8664 machine \(x64\)") {
        throw "$($pe.Name) is not a verified x64 PE image"
    }

    $dependents = (& $dumpbin /nologo /dependents $pe.FullName 2>&1) -join "`n"
    if ($LASTEXITCODE -ne 0) {
        throw "dumpbin /dependents failed for $($pe.Name)"
    }
    $rawInventory.Add("===== $($pe.Name) =====`r`n$dependents")

    $names = @([regex]::Matches($dependents, "(?im)^\s*([A-Za-z0-9_.-]+\.dll)\s*$") | ForEach-Object {
        $_.Groups[1].Value
    } | Sort-Object -Unique)
    if ($names.Count -eq 0) {
        throw "dumpbin reported no imported DLLs for $($pe.Name); refusing an incomplete inventory"
    }

    foreach ($name in $names) {
        $bundled = Join-Path $output $name
        $system32 = Join-Path $env:SystemRoot "System32/$name"
        if (Test-Path -LiteralPath $bundled -PathType Leaf) {
            $resolution = "bundled"
            $resolvedPath = $bundled
        }
        elseif ($windowsComponentDlls.Contains($name) -or $name -match "(?i)^(api|ext)-ms-win-.*\.dll$") {
            if (-not (Test-Path -LiteralPath $system32 -PathType Leaf)) {
                throw "Allowlisted Windows component '$name' imported by $($pe.Name) is absent from System32"
            }
            $resolution = "windows-system"
            $resolvedPath = $system32
        }
        elseif ($name -match "(?i)^(vcruntime|msvcp|concrt|msvcr)\d.*\.dll$") {
            $redistDll = Find-VcRuntimeDll -Name $name
            if ($null -eq $redistDll) {
                throw "Visual C++ runtime '$name' imported by $($pe.Name) is not staged and no x64 app-local redistributable copy was found"
            }
            Copy-Item -LiteralPath $redistDll.FullName -Destination $bundled
            $pendingPeFiles.Enqueue($bundled)
            $resolution = "bundled-vc-runtime"
            $resolvedPath = $bundled
        }
        else {
            throw "Native dependency '$name' imported by $($pe.Name) is neither an explicit Windows component nor staged app-local"
        }
        $imports.Add([ordered]@{
            importer = $pe.Name
            name = $name
            resolution = $resolution
            resolvedPath = $resolvedPath
        })
    }
}

$rawPath = Join-Path $output "dumpbin-dependents.txt"
[IO.File]::WriteAllText($rawPath, ($rawInventory -join "`r`n`r`n"), [Text.UTF8Encoding]::new($false))

$packagedFiles = @(Get-ChildItem -LiteralPath $output -File | Where-Object { $_.Extension -in @(".exe", ".dll") } | Sort-Object Name | ForEach-Object {
    [ordered]@{
        name = $_.Name
        bytes = $_.Length
        sha256 = Get-Sha256Lower -Path $_.FullName
    }
})
if (($packagedFiles | Where-Object { $_.name -eq [string]$manifest.packaging.binaryName }).Count -ne 1) {
    throw "Packaged file inventory does not contain exactly one gigastt.exe"
}

$forbidden = @(Get-ChildItem -LiteralPath $output -Recurse -File | Where-Object {
    $_.Extension -in @(".py", ".pyc", ".pyd") -or $_.Name -match "(?i)^python(?:3(?:\.\d+)?)?\.exe$"
})
if ($forbidden.Count -ne 0) {
    throw "Python production dependency found in runtime stage: $($forbidden.FullName -join ', ')"
}

$inventory = [ordered]@{
    schemaVersion = 1
    generatedAtUtc = [DateTime]::UtcNow.ToString("o")
    gigasttVersion = [string]$manifest.gigastt.version
    sourceCommit = $head
    sourceTag = [string]$manifest.gigastt.sourceTag
    target = [string]$manifest.toolchain.target
    rustc = $rustVersion
    protoc = $protocVersion
    cargoCommand = "cargo $($cargoArgs -join ' ')"
    tauriResourceDirectory = [string]$manifest.packaging.tauriResourceDirectory
    packagedFiles = $packagedFiles
    importedDlls = @($imports)
    pythonProductionDependency = $false
}
$inventoryPath = Join-Path $output ([string]$manifest.packaging.runtimeInventoryName)
$inventory | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $inventoryPath -Encoding utf8NoBOM

Push-Location $output
try {
    $stagedVersion = (& ".\$($manifest.packaging.binaryName)" --version).Trim()
    if ($LASTEXITCODE -ne 0 -or $stagedVersion -ne $versionOutput) {
        throw "Staged runtime cannot execute independently: got '$stagedVersion'"
    }
}
finally {
    Pop-Location
}

Write-Host ("WINDOWS BUILD PASS commit={0} binary_sha256={1} packaged_files={2} imported_dlls={3}" -f `
    $head,
    (Get-Sha256Lower -Path $stagedBinary),
    $packagedFiles.Count,
    $imports.Count)
