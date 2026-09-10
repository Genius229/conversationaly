[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Installer,
    [Parameter(Mandatory)][string]$EvidenceDir,
    [int]$TimeoutSeconds = 120,
    [switch]$TestCooperativeQuit
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $IsWindows) { throw 'Installer acceptance requires a real Windows host' }
$Installer = (Resolve-Path -LiteralPath $Installer).Path
$evidence = [IO.Path]::GetFullPath($EvidenceDir)
New-Item -ItemType Directory -Path $evidence -Force | Out-Null
$root = Join-Path ([IO.Path]::GetTempPath()) ('Conversationaly installer test ' + [guid]::NewGuid())
$install = Join-Path $root 'Installed App With Spaces'
$extracted = Join-Path $root 'payload'
$fixtures = Join-Path $root 'unrelated processes'
$owned = [Collections.Generic.List[Diagnostics.Process]]::new()
$results = [Collections.Generic.List[object]]::new()

function Save-Evidence {
    [ordered]@{
        installerSha256 = (Get-FileHash -LiteralPath $Installer -Algorithm SHA256).Hash.ToLowerInvariant()
        platform = [Environment]::OSVersion.VersionString
        cases = @($results.ToArray())
    } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $evidence 'acceptance.json') -Encoding utf8
    # Preserve the actual installed layout even when the very first hash check
    # fails. No recording/transcript contents or unrelated host files are read.
    $tree = @()
    if (Test-Path -LiteralPath $install -PathType Container) {
        $tree = @(Get-ChildItem -LiteralPath $install -Recurse -File | ForEach-Object {
            @{ path = [IO.Path]::GetRelativePath($install, $_.FullName); bytes = $_.Length; attributes = $_.Attributes.ToString() }
        })
    }
    @{ requestedInstallDir = $install; files = $tree } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $evidence 'installed-tree.json')
    $registered = @(Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*' -ErrorAction SilentlyContinue |
        Where-Object { $_.PSObject.Properties.Name -contains 'DisplayName' -and $_.DisplayName -eq 'Conversationaly GigaSTT Dev' } |
        Select-Object DisplayName, InstallLocation, UninstallString, MainBinaryName)
    $registered | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $evidence 'registered-installation.json')
}

function Start-Owned([string]$File, [string[]]$Arguments) {
    $info = [Diagnostics.ProcessStartInfo]::new($File)
    $info.UseShellExecute = $false
    foreach ($argument in $Arguments) { $info.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($info)
    $owned.Add($process)
    return $process
}

function Stop-Owned([Diagnostics.Process]$Process) {
    if (-not $Process.HasExited) {
        $Process.Kill($true)
        if (-not $Process.WaitForExit(10000)) { throw "Test process $($Process.Id) did not exit" }
    }
}

function Invoke-Installer([string]$Case, [string]$Mode = '/S') {
    $info = [Diagnostics.ProcessStartInfo]::new($Installer)
    $info.UseShellExecute = $false
    # NSIS requires /D last, unquoted even with spaces. ArgumentList would quote
    # the full /D token and NSIS would silently choose the default location.
    $info.Arguments = "$Mode /D=$install"
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $process = [Diagnostics.Process]::Start($info)
    $owned.Add($process)
    $finished = $process.WaitForExit($TimeoutSeconds * 1000)
    $entry = [ordered]@{ case = $Case; mode = $Mode; seconds = [Math]::Round($timer.Elapsed.TotalSeconds, 2); timedOut = -not $finished; exitCode = $null }
    if ($finished) { $entry.exitCode = $process.ExitCode }
    $results.Add($entry)
    Save-Evidence
    if (-not $finished) {
        Stop-Owned $process
        throw "$Case timed out (possible unattended installer dialog)"
    }
    Write-Host "$Case exit=$($process.ExitCode) seconds=$($entry.seconds)"
    return $process.ExitCode
}

function Assert-Alive([Diagnostics.Process]$Process, [string]$Description) {
    if ($Process.HasExited) { throw "Installer terminated unrelated $Description PID $($Process.Id)" }
}

function Start-Fixture([string]$Name, [string]$LockPath = '') {
    $path = Join-Path $fixtures $Name
    $ready = Join-Path $fixtures ($Name + '.ready')
    Remove-Item -LiteralPath $ready -Force -ErrorAction SilentlyContinue
    $arguments = @($ready)
    if ($LockPath) { $arguments += $LockPath }
    $process = Start-Owned $path $arguments
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while (-not (Test-Path -LiteralPath $ready)) {
        Assert-Alive $process $Name
        if ([DateTime]::UtcNow -gt $deadline) { throw "Fixture $Name never acquired its lock" }
        Start-Sleep -Milliseconds 100
    }
    return $process
}

function Get-Snapshot {
    $snapshot = [ordered]@{}
    foreach ($file in (Get-ChildItem -LiteralPath $install -Recurse -File | Sort-Object FullName)) {
        $relative = [IO.Path]::GetRelativePath($install, $file.FullName)
        $snapshot[$relative] = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash
    }
    return $snapshot
}

function Assert-Runtime {
    $files = @($script:inventory.packagedFiles)
    if ($files.Count -lt 6) { throw 'Runtime inventory is empty or incomplete' }
    $names = @{}
    foreach ($file in $files) {
        if ($file.name -ne [IO.Path]::GetFileName($file.name) -or $file.name -match '[:/\\]' -or $names.ContainsKey($file.name)) {
            throw 'Invalid/duplicate runtime inventory path'
        }
        $names[$file.name] = $true
        $path = Join-Path $install (Join-Path 'gigastt' $file.name)
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing installed runtime file $($file.name)" }
        if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $file.sha256) {
            throw "Installed runtime hash mismatch: $($file.name)"
        }
    }
    foreach ($required in @('gigastt.exe', 'DirectML.dll', 'MSVCP140.dll', 'MSVCP140_1.dll', 'VCRUNTIME140.dll', 'VCRUNTIME140_1.dll')) {
        if (-not $names.ContainsKey($required)) { throw "Inventory omits $required" }
    }
    $installedBinaries = @(Get-ChildItem -LiteralPath (Join-Path $install 'gigastt') -File | Where-Object { $_.Extension -in @('.exe', '.dll') })
    if ($installedBinaries.Count -ne $names.Count) { throw 'Installed runtime contains uninventoried or stale binaries' }
    foreach ($binary in $installedBinaries) {
        if (-not $names.ContainsKey($binary.Name)) { throw "Runtime binary not covered by inventory: $($binary.Name)" }
    }
}

try {
    New-Item -ItemType Directory -Path $fixtures, $extracted -Force | Out-Null
    & 7z x $Installer "-o$extracted" -y | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Could not extract installer' }
    $inventories = @(Get-ChildItem -LiteralPath $extracted -Recurse -File -Filter runtime-inventory.json)
    if ($inventories.Count -ne 1) { throw 'Expected exactly one packaged runtime inventory' }
    $script:inventory = Get-Content -LiteralPath $inventories[0].FullName -Raw | ConvertFrom-Json
    Copy-Item -LiteralPath $inventories[0].FullName -Destination (Join-Path $evidence 'runtime-inventory.json')
    $mainPayload = @(Get-ChildItem -LiteralPath $extracted -Recurse -File -Filter conversationaly.exe)
    if ($mainPayload.Count -ne 1) { throw 'Expected exactly one packaged conversationaly.exe' }
    $mainHash = (Get-FileHash -LiteralPath $mainPayload[0].FullName -Algorithm SHA256).Hash

    # Data/models live outside the installation root. A uniquely named sentinel
    # exercises preservation without changing real preferences or model files.
    $data = Join-Path $env:APPDATA 'com.conversationaly.gigastt-dev'
    $sentinel = Join-Path $data ('models\installer-test-' + [guid]::NewGuid() + '.sentinel')
    New-Item -ItemType Directory -Path (Split-Path -Parent $sentinel) -Force | Out-Null
    [IO.File]::WriteAllText($sentinel, 'keep-model-data')

    if ((Invoke-Installer 'fresh-install') -ne 0) { throw 'Fresh installation failed' }
    Assert-Runtime
    $main = Join-Path $install 'conversationaly.exe'
    if ((Get-FileHash -LiteralPath $main -Algorithm SHA256).Hash -ne $mainHash) { throw 'Fresh main binary mismatch' }

    $compiler = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
    & $compiler /nologo /target:exe "/out:$fixtures\gigastt.exe" (Join-Path $PSScriptRoot 'LockFixture.cs')
    if ($LASTEXITCODE -ne 0) { throw 'Could not compile process/lock fixture' }
    Copy-Item -LiteralPath (Join-Path $fixtures 'gigastt.exe') -Destination (Join-Path $fixtures 'conversationaly.exe')
    $unrelated = Start-Fixture 'conversationaly.exe'
    $holder = Start-Fixture 'gigastt.exe' (Join-Path $install 'gigastt\DirectML.dll')

    # Ensure a premature copy is observable even when updating to the same build.
    $stream = [IO.File]::Open($main, [IO.FileMode]::Append, [IO.FileAccess]::Write)
    try { $marker = [Text.Encoding]::ASCII.GetBytes('installer-upgrade-sentinel'); $stream.Write($marker, 0, $marker.Length) }
    finally { $stream.Dispose() }
    $before = Get-Snapshot
    $before | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $evidence 'before-locked-upgrade.json')
    $code = Invoke-Installer 'locked-upgrade'
    $after = Get-Snapshot
    $after | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $evidence 'after-locked-upgrade.json')
    Assert-Alive $holder 'GigaSTT file holder'
    Assert-Alive $unrelated 'Conversationaly process'
    if ($code -ne 12) { throw "Locked upgrade must fail preflight with code 12, got $code" }
    if (($before | ConvertTo-Json -Compress) -cne ($after | ConvertTo-Json -Compress)) { throw 'Blocked update removed or changed installed files' }

    # Tauri updater defaults to passive, not silent. Also cover /P WITHOUT
    # /UPDATE: it traverses the maintenance/old-uninstaller path in stock NSIS.
    # None may display a Retry/Ignore dialog or remove the existing payload.
    foreach ($mode in @('/S /R /UPDATE', '/P /R /UPDATE', '/P')) {
        $case = 'locked-' + $mode.Replace('/', '').Replace(' ', '-')
        if ((Invoke-Installer $case $mode) -ne 12) { throw "Locked updater failed its preflight contract: $mode" }
        Assert-Alive $holder 'GigaSTT file holder'
        Assert-Alive $unrelated 'Conversationaly process'
        $current = Get-Snapshot
        if (($before | ConvertTo-Json -Compress) -cne ($current | ConvertTo-Json -Compress)) { throw "Blocked updater changed payload: $mode" }
        if ([IO.File]::ReadAllText($sentinel) -cne 'keep-model-data') { throw "Blocked updater changed model data: $mode" }
    }

    Stop-Owned $holder
    $holder = Start-Fixture 'gigastt.exe' (Join-Path $install 'gigastt\VCRUNTIME140.dll')
    if ((Invoke-Installer 'locked-msvc-upgrade') -ne 12) { throw 'Locked MSVC runtime must fail preflight with code 12' }
    Assert-Alive $holder 'MSVC file holder'
    Assert-Alive $unrelated 'Conversationaly process'
    $afterMsvc = Get-Snapshot
    if (($before | ConvertTo-Json -Compress) -cne ($afterMsvc | ConvertTo-Json -Compress)) { throw 'Blocked MSVC update changed payload' }
    Stop-Owned $holder
    if ((Invoke-Installer 'retry-after-unlock') -ne 0) { throw 'Upgrade after releasing DLL lock failed' }
    Assert-Alive $unrelated 'Conversationaly process'
    Assert-Runtime
    if ((Get-FileHash -LiteralPath $main -Algorithm SHA256).Hash -ne $mainHash) { throw 'Successful upgrade did not restore the main payload' }
    if ([IO.File]::ReadAllText($sentinel) -cne 'keep-model-data') { throw 'Update changed application model data' }
    Stop-Owned $unrelated

    $readOnlyFile = Join-Path $install 'gigastt\MSVCP140.dll'
    $originalAttributes = [IO.File]::GetAttributes($readOnlyFile)
    try {
        [IO.File]::SetAttributes($readOnlyFile, $originalAttributes -bor [IO.FileAttributes]::ReadOnly)
        $beforeReadOnly = Get-Snapshot
        if ((Invoke-Installer 'read-only-destination') -ne 12) { throw 'Read-only destination must fail with code 12' }
        $afterReadOnly = Get-Snapshot
        if (($beforeReadOnly | ConvertTo-Json -Compress) -cne ($afterReadOnly | ConvertTo-Json -Compress)) { throw 'Read-only failure changed installed files' }
        if (-not ([IO.File]::GetAttributes($readOnlyFile) -band [IO.FileAttributes]::ReadOnly)) { throw 'Installer silently changed destination attributes' }
    } finally {
        [IO.File]::SetAttributes($readOnlyFile, $originalAttributes)
    }

    if ($TestCooperativeQuit) {
        $app = Start-Owned $main @()
        Start-Sleep -Seconds 12
        Assert-Alive $app 'installed app before cooperative quit test'
        if ((Invoke-Installer 'running-app-cooperative-upgrade') -ne 0) { throw 'Cooperative app upgrade failed' }
        if (-not $app.WaitForExit(10000)) { throw 'Installed application did not cooperate with installer quit' }
        Assert-Runtime
    }
    if ([IO.File]::ReadAllText($sentinel) -cne 'keep-model-data') { throw 'Installation tests changed model data' }
    $results.Add([ordered]@{ case = 'all-assertions'; passed = $true })
    Write-Host 'WINDOWS INSTALLER ACCEPTANCE PASS'
} catch {
    $results.Add([ordered]@{ case = 'failure'; message = $_.Exception.Message })
    throw
} finally {
    Save-Evidence
    foreach ($process in $owned) { Stop-Owned $process }
    # Only our generated sentinel is deleted, never the user's data directory.
    if (Get-Variable sentinel -ErrorAction SilentlyContinue) { Remove-Item -LiteralPath $sentinel -Force -ErrorAction SilentlyContinue }
    # The installation remains in this ephemeral runner temp directory for
    # failure diagnostics; no broad process cleanup or old uninstaller is used.
}
