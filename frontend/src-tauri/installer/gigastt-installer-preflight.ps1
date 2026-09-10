[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("Preflight", "Deploy")]
    [string]$Mode,

    [Parameter(Mandatory = $true)]
    [string]$InstallDir,

    [Parameter(Mandatory = $true)]
    [string]$MainBinaryName,

    [string]$PayloadDir = "",

    [ValidateRange(1, 120)]
    [int]$WaitSeconds = 30
)

# Exit contract consumed by gigastt-installer-hooks.nsh and the Windows
# acceptance test. Keep each failure before deployment starts.
function Exit-MainBusy([string]$Message) {
    [Console]::Out.WriteLine($Message)
    exit 10
}

function Exit-SidecarBusy([string]$Message) {
    [Console]::Out.WriteLine($Message)
    exit 11
}

function Exit-TargetLocked([string]$Message) {
    [Console]::Out.WriteLine($Message)
    exit 12
}

function Exit-Unsafe([string]$Message) {
    [Console]::Out.WriteLine($Message)
    exit 13
}

Set-StrictMode -Version 2.0
$ErrorActionPreference = "Stop"

function Get-NormalizedPath([string]$Path) {
    $fullPath = [IO.Path]::GetFullPath($Path)
    $extendedUncPrefix = "\\?\UNC\"
    $extendedPrefix = "\\?\"
    if ($fullPath.StartsWith($extendedUncPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        $fullPath = "\\" + $fullPath.Substring(8)
    }
    elseif ($fullPath.StartsWith($extendedPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        $fullPath = $fullPath.Substring(4)
    }
    return $fullPath.TrimEnd([char[]]@(92, 47))
}

function Test-SamePath([string]$Left, [string]$Right) {
    if ([string]::IsNullOrWhiteSpace($Left) -or [string]::IsNullOrWhiteSpace($Right)) {
        return $false
    }

    try {
        $normalizedLeft = Get-NormalizedPath $Left
        $normalizedRight = Get-NormalizedPath $Right
        return [string]::Equals(
            $normalizedLeft,
            $normalizedRight,
            [StringComparison]::OrdinalIgnoreCase
        )
    }
    catch {
        return $false
    }
}

function Get-ExactPathProcesses([string]$ExecutablePath) {
    $matches = @()
    foreach ($process in @(Get-Process -ErrorAction SilentlyContinue)) {
        try {
            $processPath = $process.Path
        }
        catch {
            continue
        }

        if (Test-SamePath $processPath $ExecutablePath) {
            $matches += $process
        }
    }
    return @($matches)
}

function Wait-ForExactPathExit([string]$ExecutablePath, [DateTime]$Deadline) {
    while ([DateTime]::UtcNow -lt $Deadline) {
        if (@(Get-ExactPathProcesses $ExecutablePath).Count -eq 0) {
            return $true
        }
        Start-Sleep -Milliseconds 200
    }
    return @(Get-ExactPathProcesses $ExecutablePath).Count -eq 0
}

function Request-MainShutdown([string]$MainPath) {
    $running = @(Get-ExactPathProcesses $MainPath)
    $capturedPids = @($running | ForEach-Object { [int]$_.Id })
    if ($running.Count -eq 0) {
        return @($capturedPids)
    }

    try {
        # The second instance is routed by tauri-plugin-single-instance to the
        # bundle owner. The explicit target plus argv[0] let the receiver prove
        # it is also the exact installed binary before acting. Compatible
        # versions exit only when no recording is active; old versions remain.
        $argumentLine = '--installer-quit --installer-target="' + $MainPath + '"'
        $request = Start-Process `
            -FilePath $MainPath `
            -ArgumentList $argumentLine `
            -PassThru
        if ($null -ne $request) {
            [void]$request.WaitForExit([Math]::Min(5000, $WaitSeconds * 1000))
        }
    }
    catch {
        Exit-MainBusy (
            "Conversationaly is running from this installation, but setup could not request " +
            "a safe shutdown. Stop and save any recording, choose Quit from the tray, then Retry."
        )
    }

    $deadline = [DateTime]::UtcNow.AddSeconds($WaitSeconds)
    if (-not (Wait-ForExactPathExit $MainPath $deadline)) {
        Exit-MainBusy (
            "Conversationaly is still running from this installation. If a recording is active, " +
            "stop and save it first; otherwise choose Quit from the tray. Then Retry."
        )
    }

    return @($capturedPids)
}

function Get-ParentProcessId([int]$ProcessId) {
    try {
        $record = Get-CimInstance `
            -ClassName Win32_Process `
            -Filter "ProcessId = $ProcessId" `
            -ErrorAction Stop
        if ($null -eq $record) {
            return $null
        }
        return [int]$record.ParentProcessId
    }
    catch {
        return $null
    }
}

function Test-ProcessExists([int]$ProcessId) {
    return $null -ne (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue)
}

function Stop-AttributableSidecars(
    [string]$InstallRoot,
    [int[]]$CapturedMainPids
) {
    # Every candidate is a literal path inside this install. A process with the
    # same basename elsewhere is never selected or stopped.
    $candidates = @(
        (Join-Path $InstallRoot "gigastt\gigastt.exe"),
        (Join-Path $InstallRoot "llama-helper.exe"),
        (Join-Path $InstallRoot "llama-helper-x86_64-pc-windows-msvc.exe")
    )

    foreach ($candidate in $candidates) {
        foreach ($process in @(Get-ExactPathProcesses $candidate)) {
            $parentProcessId = Get-ParentProcessId ([int]$process.Id)
            $ownedByCapturedMain =
                $null -ne $parentProcessId -and
                $CapturedMainPids -contains $parentProcessId
            $orphaned =
                $null -ne $parentProcessId -and
                -not (Test-ProcessExists $parentProcessId)

            if (-not $ownedByCapturedMain -and -not $orphaned) {
                Exit-SidecarBusy (
                    "A sidecar is running from this installation but setup cannot safely prove " +
                    "that it is orphaned: $candidate. Close its owning Conversationaly instance, then Retry."
                )
            }

            # GigaSTT has no Windows shutdown endpoint. At this point the exact
            # parent app exited safely (or no longer exists), so stop only this PID.
            try {
                Stop-Process -Id ([int]$process.Id) -Force -ErrorAction Stop
                [void]$process.WaitForExit(5000)
            }
            catch {
                Exit-SidecarBusy (
                    "Setup could not stop the orphaned sidecar from this installation: $candidate. " +
                    "Close it, then Retry."
                )
            }

            if (@(Get-ExactPathProcesses $candidate).Count -ne 0) {
                Exit-SidecarBusy (
                    "The orphaned sidecar from this installation did not exit in time: $candidate. " +
                    "Close it, then Retry."
                )
            }
        }
    }
}

function Assert-DirectoryWritable([string]$Directory) {
    $probePath = Join-Path $Directory (".conversationaly-installer-probe-" + [Guid]::NewGuid().ToString("N") + ".tmp")
    try {
        $probe = [IO.File]::Open(
            $probePath,
            [IO.FileMode]::CreateNew,
            [IO.FileAccess]::ReadWrite,
            [IO.FileShare]::None
        )
        $probe.Dispose()
        Remove-Item -LiteralPath $probePath -Force -ErrorAction Stop
    }
    catch {
        if (Test-Path -LiteralPath $probePath) {
            Remove-Item -LiteralPath $probePath -Force -ErrorAction SilentlyContinue
        }
        Exit-TargetLocked (
            "Setup cannot write to '$Directory'. Choose a writable location or fix the folder " +
            "permissions, then Retry."
        )
    }
}

function Assert-TargetReady([string]$InstallRoot) {
    $handles = New-Object System.Collections.Generic.List[System.IDisposable]
    try {
        if (Test-Path -LiteralPath $InstallRoot -PathType Leaf) {
            Exit-Unsafe "The selected install path is a file, not a directory: $InstallRoot"
        }

        if (Test-Path -LiteralPath $InstallRoot -PathType Container) {
            foreach ($file in @(Get-ChildItem -LiteralPath $InstallRoot -File -Recurse -Force)) {
                if (($file.Attributes -band [IO.FileAttributes]::ReadOnly) -ne 0) {
                    Exit-TargetLocked (
                        "Setup cannot safely replace '$($file.FullName)' because it is read-only. " +
                        "Make the file writable, then Retry."
                    )
                }
                try {
                    $handle = [IO.File]::Open(
                        $file.FullName,
                        [IO.FileMode]::Open,
                        [IO.FileAccess]::ReadWrite,
                        [IO.FileShare]::None
                    )
                    $handles.Add($handle)
                }
                catch {
                    Exit-TargetLocked (
                        "Setup cannot safely replace '$($file.FullName)'. It is locked or not writable. " +
                        "Close the owning program, then Retry."
                    )
                }
            }

            # Moving a payload file requires write/delete rights on its parent,
            # which opening the file itself does not prove. Probe every existing
            # install directory without changing ACLs or leaving a file behind.
            Assert-DirectoryWritable $InstallRoot
            foreach ($directory in @(
                Get-ChildItem -LiteralPath $InstallRoot -Directory -Recurse -Force
            )) {
                Assert-DirectoryWritable $directory.FullName
            }
        }

        $parent = Split-Path -Parent $InstallRoot
        if ([string]::IsNullOrWhiteSpace($parent)) {
            Exit-Unsafe "The selected install path has no safe parent directory: $InstallRoot"
        }
        [IO.Directory]::CreateDirectory($parent) | Out-Null
        Assert-DirectoryWritable $parent
    }
    finally {
        foreach ($handle in $handles) {
            $handle.Dispose()
        }
    }
}

function Copy-PayloadToSiblingStage(
    [string]$SourceRoot,
    [string]$StageRoot
) {
    [IO.Directory]::CreateDirectory($StageRoot) | Out-Null
    foreach ($entry in @(Get-ChildItem -LiteralPath $SourceRoot -Recurse -Force)) {
        $relative = $entry.FullName.Substring($SourceRoot.Length).TrimStart([char[]]@(92, 47))
        $destination = Join-Path $StageRoot $relative
        if ($entry.PSIsContainer) {
            [IO.Directory]::CreateDirectory($destination) | Out-Null
        }
        else {
            $destinationParent = Split-Path -Parent $destination
            [IO.Directory]::CreateDirectory($destinationParent) | Out-Null
            [IO.File]::Copy($entry.FullName, $destination, $false)
        }
    }
}

function Assert-StagedRuntimeComplete([string]$SourceRoot) {
    $runtimeRoot = Join-Path $SourceRoot "gigastt"
    $inventoryPath = Join-Path $runtimeRoot "runtime-inventory.json"
    if (-not (Test-Path -LiteralPath $inventoryPath -PathType Leaf)) {
        Exit-Unsafe "The staged GigaSTT runtime inventory is missing. Setup did not change the installation."
    }

    try {
        $inventory = Get-Content -LiteralPath $inventoryPath -Raw -ErrorAction Stop |
            ConvertFrom-Json -ErrorAction Stop
        $packagedFiles = @($inventory.packagedFiles)
    }
    catch {
        Exit-Unsafe (
            "The staged GigaSTT runtime inventory is invalid. Setup did not change the installation: " +
            $_.Exception.Message
        )
    }
    if ($packagedFiles.Count -eq 0) {
        Exit-Unsafe "The staged GigaSTT runtime inventory is empty. Setup did not change the installation."
    }

    $names = @{}
    foreach ($file in $packagedFiles) {
        $name = [string]$file.name
        if (
            [string]::IsNullOrWhiteSpace($name) -or
            [IO.Path]::IsPathRooted($name) -or
            [IO.Path]::GetFileName($name) -ne $name -or
            $name.Contains("\") -or
            $name.Contains("/") -or
            $name -eq "." -or
            $name -eq ".." -or
            $names.ContainsKey($name)
        ) {
            Exit-Unsafe "The staged GigaSTT runtime inventory contains an unsafe or duplicate file name."
        }
        $names[$name] = $true

        $path = Join-Path $runtimeRoot $name
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            Exit-Unsafe (
                "The staged GigaSTT runtime is incomplete: '$name' is missing. " +
                "Setup did not change the installation."
            )
        }
    }
}

function Deploy-PayloadTransactionally(
    [string]$InstallRoot,
    [string]$SourceRoot,
    [string]$ExpectedMainBinaryName
) {
    if (-not (Test-Path -LiteralPath $SourceRoot -PathType Container)) {
        Exit-Unsafe "The staged installer payload is missing. Setup did not change the installation."
    }
    if (-not (Test-Path -LiteralPath (Join-Path $SourceRoot $ExpectedMainBinaryName) -PathType Leaf)) {
        Exit-Unsafe "The staged main executable is missing. Setup did not change the installation."
    }
    Assert-StagedRuntimeComplete $SourceRoot

    $parent = Split-Path -Parent $InstallRoot
    $transactionId = [Guid]::NewGuid().ToString("N")
    $stageRoot = Join-Path $parent (".conversationaly-new-" + $transactionId)
    $backupRoot = Join-Path $parent (".conversationaly-backup-" + $transactionId)
    $movedOld = New-Object System.Collections.Generic.List[string]
    $movedNew = New-Object System.Collections.Generic.List[string]
    $committed = $false

    try {
        Copy-PayloadToSiblingStage $SourceRoot $stageRoot
        $payloadFiles = @(
            Get-ChildItem -LiteralPath $stageRoot -File -Recurse -Force |
                Sort-Object FullName
        )
        $payloadPathSet = @{}
        foreach ($file in $payloadFiles) {
            $relative = $file.FullName.Substring($stageRoot.Length).TrimStart([char[]]@(92, 47))
            $payloadPathSet[$relative] = $true
        }

        [IO.Directory]::CreateDirectory($InstallRoot) | Out-Null
        [IO.Directory]::CreateDirectory($backupRoot) | Out-Null

        $filesToBackup = New-Object System.Collections.Generic.List[string]
        foreach ($file in $payloadFiles) {
            $relative = $file.FullName.Substring($stageRoot.Length).TrimStart([char[]]@(92, 47))
            $target = Join-Path $InstallRoot $relative
            if (Test-Path -LiteralPath $target -PathType Leaf) {
                $filesToBackup.Add($relative)
            }
        }

        # Only the old signed build inventory proves which pre-existing runtime
        # paths are installer-owned. Unknown files under gigastt are preserved;
        # downloaded models and recordings are never part of this inventory.
        $managedRuntimeRoot = Join-Path $InstallRoot "gigastt"
        $oldInventoryPath = Join-Path $managedRuntimeRoot "runtime-inventory.json"
        if (Test-Path -LiteralPath $oldInventoryPath -PathType Leaf) {
            try {
                $oldInventory = Get-Content -LiteralPath $oldInventoryPath -Raw -ErrorAction Stop |
                    ConvertFrom-Json -ErrorAction Stop
                $oldPackagedFiles = @($oldInventory.packagedFiles)
            }
            catch {
                Exit-Unsafe (
                    "The installed GigaSTT runtime inventory is invalid. Setup preserved every " +
                    "runtime file: $($_.Exception.Message)"
                )
            }

            foreach ($oldFile in $oldPackagedFiles) {
                $oldName = [string]$oldFile.name
                if (
                    [string]::IsNullOrWhiteSpace($oldName) -or
                    [IO.Path]::IsPathRooted($oldName) -or
                    [IO.Path]::GetFileName($oldName) -ne $oldName -or
                    $oldName.Contains("\") -or
                    $oldName.Contains("/") -or
                    $oldName -eq "." -or
                    $oldName -eq ".."
                ) {
                    Exit-Unsafe (
                        "The installed GigaSTT runtime inventory contains an unsafe file name. " +
                        "Setup preserved every runtime file."
                    )
                }

                $relative = Join-Path "gigastt" $oldName
                $target = Join-Path $InstallRoot $relative
                if (
                    -not $payloadPathSet.ContainsKey($relative) -and
                    (Test-Path -LiteralPath $target -PathType Leaf)
                ) {
                    $filesToBackup.Add($relative)
                }
            }
        }

        # Move every colliding or obsolete installer-owned file away before
        # publishing any new one. A failure rolls all earlier moves back.
        foreach ($relative in $filesToBackup) {
            $target = Join-Path $InstallRoot $relative
            $backup = Join-Path $backupRoot $relative
            [IO.Directory]::CreateDirectory((Split-Path -Parent $backup)) | Out-Null
            [IO.File]::Move($target, $backup)
            $movedOld.Add($relative)
        }

        # Obsolete empty directories are harmless and deliberately retained
        # rather than broad-deleting an installer-owned tree.

        # Publish the complete staged payload. Any failure removes published
        # new files and restores every old file from the backup tree.
        foreach ($file in $payloadFiles) {
            $relative = $file.FullName.Substring($stageRoot.Length).TrimStart([char[]]@(92, 47))
            $target = Join-Path $InstallRoot $relative
            [IO.Directory]::CreateDirectory((Split-Path -Parent $target)) | Out-Null
            [IO.File]::Move($file.FullName, $target)
            $movedNew.Add($relative)
        }

        $committed = $true
    }
    catch {
        $deploymentError = $_.Exception.Message
        $rollbackErrors = New-Object System.Collections.Generic.List[string]

        for ($index = $movedNew.Count - 1; $index -ge 0; $index--) {
            $target = Join-Path $InstallRoot $movedNew[$index]
            try {
                if (Test-Path -LiteralPath $target -PathType Leaf) {
                    [IO.File]::Delete($target)
                }
            }
            catch {
                $rollbackErrors.Add($_.Exception.Message)
            }
        }

        for ($index = $movedOld.Count - 1; $index -ge 0; $index--) {
            $relative = $movedOld[$index]
            $backup = Join-Path $backupRoot $relative
            $target = Join-Path $InstallRoot $relative
            try {
                [IO.Directory]::CreateDirectory((Split-Path -Parent $target)) | Out-Null
                if (Test-Path -LiteralPath $backup -PathType Leaf) {
                    [IO.File]::Move($backup, $target)
                }
            }
            catch {
                $rollbackErrors.Add($_.Exception.Message)
            }
        }

        if ($rollbackErrors.Count -ne 0) {
            Exit-Unsafe (
                "Setup deployment failed and rollback was incomplete: $deploymentError; " +
                ($rollbackErrors -join "; ")
            )
        }
        Exit-TargetLocked (
            "Setup could not publish the staged payload, and restored the previous installation: " +
            "$deploymentError. Close the owning program, then Retry."
        )
    }
    finally {
        if (Test-Path -LiteralPath $stageRoot) {
            Remove-Item -LiteralPath $stageRoot -Recurse -Force -ErrorAction SilentlyContinue
        }
        if ($committed -and (Test-Path -LiteralPath $backupRoot)) {
            Remove-Item -LiteralPath $backupRoot -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}

try {
    $installRoot = Get-NormalizedPath $InstallDir
    $root = [IO.Path]::GetPathRoot($installRoot)
    if (
        [string]::IsNullOrWhiteSpace($installRoot) -or
        [string]::IsNullOrWhiteSpace($root) -or
        (Test-SamePath $installRoot $root)
    ) {
        Exit-Unsafe "Setup refused the unsafe install path '$InstallDir'."
    }

    if (
        [IO.Path]::GetFileName($MainBinaryName) -ne $MainBinaryName -or
        -not $MainBinaryName.EndsWith(".exe", [StringComparison]::OrdinalIgnoreCase)
    ) {
        Exit-Unsafe "Setup received an unsafe main executable name."
    }

    $mainPath = Join-Path $installRoot $MainBinaryName
    [int[]]$mainPids = @(Request-MainShutdown $mainPath)
    Stop-AttributableSidecars $installRoot $mainPids
    Assert-TargetReady $installRoot

    if ($Mode -eq "Deploy") {
        $payloadRoot = Get-NormalizedPath $PayloadDir
        Deploy-PayloadTransactionally $installRoot $payloadRoot $MainBinaryName
    }

    [Console]::Out.WriteLine("Conversationaly installer preflight ready.")
    exit 0
}
catch {
    Exit-Unsafe ("Installer preflight failed before setup could continue: " + $_.Exception.Message)
}
