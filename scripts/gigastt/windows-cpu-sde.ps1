# Pure assertions and bounded process runner for CI-only Intel SDE validation.
# The Intel tool is never redistributed in the GigaSTT runtime package.
function Get-IvbArguments {
    param([Parameter(Mandatory=$true)][string]$Binary,
          [Parameter(Mandatory=$true)][string[]]$Arguments,
          [Parameter(Mandatory=$true)][string[]]$Images)
    $binaryName = ($Binary -split '[\\/]')[-1]
    if ($binaryName -notin $Images) { throw 'Executable missing from instruction-check scope' }
    $result = @('-ivb','-chip_check_call_stack','1')
    foreach ($name in $Images) {
        if ($name -notmatch '^[A-Za-z0-9_.-]+\.(exe|dll)$') { throw 'Invalid image selector' }
        $result += @('-chip_check_image', $name)
    }
    return [string[]]@($result + @('--', $Binary) + $Arguments)
}

function Assert-IvbOutcome {
    param([int]$ExitCode, [AllowEmptyString()][string]$Text, [switch]$ExpectedIllegalInstruction)
    if ($ExpectedIllegalInstruction) {
        if ($ExitCode -eq 0 -or $Text -notmatch 'SDE-ERROR:\s*Executed instruction not valid for specified chip') {
            throw "Negative control did not fail the IVB instruction check (exit $ExitCode)"
        }
    } elseif ($ExitCode -ne 0 -or $Text -match 'SDE-ERROR') {
        throw "Candidate failed the IVB instruction check (exit $ExitCode)"
    }
}

function Assert-IvbTranscript {
    param([Parameter(Mandatory=$true)]$Result)
    # GigaSTT CLI export/mod.rs::to_json serializes text, words, duration.
    # It does NOT contain the jobs API's segments member.
    if ([string]::IsNullOrWhiteSpace([string]$Result.text) -or [string]$Result.text -notmatch '[\u0400-\u04ff]' -or @($Result.words).Count -eq 0) { throw 'Invalid real-model transcript' }
    $duration = [double]$Result.duration
    if ([double]::IsNaN($duration) -or [double]::IsInfinity($duration) -or $duration -le 0) { throw 'Invalid transcript duration' }
}

function Invoke-BoundedSde {
    param([Parameter(Mandatory=$true)][string]$Sde,
          [Parameter(Mandatory=$true)][string[]]$Arguments,
          [Parameter(Mandatory=$true)][string]$WorkingDirectory,
          [Parameter(Mandatory=$true)][string]$EvidencePrefix,
          [ValidateRange(1,3600)][int]$TimeoutSeconds = 1200)
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Sde
    $start.WorkingDirectory = $WorkingDirectory
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in $Arguments) { $start.ArgumentList.Add($argument) }
    # Do not let local overrides replace the exact runtime/model/profile tested.
    foreach ($name in @($start.Environment.Keys)) {
        if ($name -like 'GIGASTT_*' -or $name -eq 'ORT_DYLIB_PATH') { [void]$start.Environment.Remove($name) }
    }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $start
    $started = $false
    $clock = [Diagnostics.Stopwatch]::StartNew()
    try {
        if (-not $process.Start()) { throw 'SDE failed to start' }
        $started = $true
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        $timedOut = -not $process.WaitForExit($TimeoutSeconds * 1000)
        if ($timedOut) {
            $process.Kill($true)
            if (-not $process.WaitForExit(10000)) { throw 'SDE process-tree cleanup failed after timeout' }
        }
        if (-not [Threading.Tasks.Task]::WaitAll([Threading.Tasks.Task[]]@($stdout, $stderr), 10000)) { throw 'SDE output drain deadline exceeded' }
        [IO.File]::WriteAllText("$EvidencePrefix.stdout.log", $stdout.Result)
        [IO.File]::WriteAllText("$EvidencePrefix.stderr.log", $stderr.Result)
        if ($timedOut) { throw "SDE exceeded deadline ($TimeoutSeconds seconds)" }
        return [pscustomobject]@{ exitCode=$process.ExitCode; durationMilliseconds=$clock.ElapsedMilliseconds; text=($stdout.Result + "`n" + $stderr.Result) }
    } finally {
        if ($started -and -not $process.HasExited) { $process.Kill($true); [void]$process.WaitForExit(10000) }
        $process.Dispose()
    }
}
