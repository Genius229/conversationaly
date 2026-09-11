[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Binary,
    [Parameter(Mandatory = $true)][string]$Audio,
    [Parameter(Mandatory = $true)][string]$ModelDir,
    [Parameter(Mandatory = $true)][string]$ExpectedAudioSha256,
    [string]$PunctModelDir,
    [string]$VadModelDir,
    [string]$BaseUrl = "http://127.0.0.1:9876",
    [string]$EvidenceDir = (Join-Path $env:TEMP "gigastt-windows-smoke"),
    [string]$RawResultPath = "",
    [bool]$RequestItn = $true,
    [int]$StartupTimeoutSeconds = 180,
    [int]$JobTimeoutSeconds = 180,
    [int]$RequestTimeoutSeconds = 15,
    [int]$ShutdownTimeoutSeconds = 20
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

function Assert-PositiveTimeout {
    param([int]$Value, [string]$Name)
    if ($Value -le 0) {
        throw "$Name must be greater than zero"
    }
}

function ConvertTo-FiniteDouble {
    param($Value, [string]$Name)
    if ($null -eq $Value) {
        throw "$Name is missing"
    }
    try {
        $number = [Convert]::ToDouble($Value, [Globalization.CultureInfo]::InvariantCulture)
    }
    catch {
        throw "$Name is not numeric"
    }
    if ([double]::IsNaN($number) -or [double]::IsInfinity($number)) {
        throw "$Name is not finite"
    }
    return $number
}

function Get-StringSha256 {
    param([Parameter(Mandatory = $true)][string]$Value)
    $bytes = [Text.Encoding]::UTF8.GetBytes($Value)
    $hash = [Security.Cryptography.SHA256]::HashData($bytes)
    return [Convert]::ToHexString($hash).ToLowerInvariant()
}

function Add-Failure {
    param([string]$Current, [string]$Next)
    if ([string]::IsNullOrWhiteSpace($Current)) {
        return $Next
    }
    return "$Current; $Next"
}

if (-not $IsWindows) {
    throw "windows-smoke.ps1 must run on native Windows"
}
foreach ($timeout in @(
    @{ Value = $StartupTimeoutSeconds; Name = "StartupTimeoutSeconds" },
    @{ Value = $JobTimeoutSeconds; Name = "JobTimeoutSeconds" },
    @{ Value = $RequestTimeoutSeconds; Name = "RequestTimeoutSeconds" },
    @{ Value = $ShutdownTimeoutSeconds; Name = "ShutdownTimeoutSeconds" }
)) {
    Assert-PositiveTimeout -Value $timeout.Value -Name $timeout.Name
}

$binaryPath = (Resolve-Path -LiteralPath $Binary).Path
$audioPath = (Resolve-Path -LiteralPath $Audio).Path
$modelPath = (Resolve-Path -LiteralPath $ModelDir).Path
if ([string]::IsNullOrWhiteSpace($PunctModelDir)) {
    $PunctModelDir = Join-Path $modelPath "punct"
}
if ([string]::IsNullOrWhiteSpace($VadModelDir)) {
    $VadModelDir = Join-Path $modelPath "vad"
}
$punctPath = (Resolve-Path -LiteralPath $PunctModelDir).Path
$vadPath = (Resolve-Path -LiteralPath $VadModelDir).Path

$uri = [Uri]$BaseUrl
if ($uri.Scheme -ne "http" -or $uri.AbsolutePath -ne "/" -or $uri.Host -notin @("127.0.0.1", "localhost", "::1")) {
    throw "BaseUrl must be an HTTP loopback origin without a path"
}

$actualAudioSha = (Get-FileHash -LiteralPath $audioPath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actualAudioSha -ne $ExpectedAudioSha256.ToLowerInvariant()) {
    throw "Russian fixture SHA-256 mismatch: expected $ExpectedAudioSha256, got $actualAudioSha"
}

New-Item -ItemType Directory -Path $EvidenceDir -Force | Out-Null
$stdoutPath = Join-Path $EvidenceDir "gigastt.stdout.log"
$stderrPath = Join-Path $EvidenceDir "gigastt.stderr.log"
$summaryPath = Join-Path $EvidenceDir "smoke-evidence.json"

$alreadyListening = $false
try {
    Invoke-RestMethod -Uri "$BaseUrl/health" -Method Get -TimeoutSec 2 | Out-Null
    $alreadyListening = $true
}
catch {
    $alreadyListening = $false
}
if ($alreadyListening) {
    throw "Refusing to smoke against a pre-existing service at $BaseUrl"
}

$arguments = @(
    "--offline",
    "serve",
    "--host", $uri.Host,
    "--port", [string]$uri.Port,
    "--model-dir", $modelPath,
    "--model-variant", "rnnt",
    "--pool-size", "1",
    "--enable-jobs",
    "--shutdown-drain-secs", "10",
    "--vad",
    "--vad-model-dir", $vadPath,
    "--punctuation", "on",
    "--punct-model-dir", $punctPath,
    "--itn", "on"
)

$startInfo = [Diagnostics.ProcessStartInfo]::new()
$startInfo.FileName = $binaryPath
$startInfo.WorkingDirectory = Split-Path -Parent $binaryPath
$startInfo.UseShellExecute = $false
$startInfo.RedirectStandardOutput = $true
$startInfo.RedirectStandardError = $true
$startInfo.CreateNoWindow = $false
foreach ($argument in $arguments) {
    $startInfo.ArgumentList.Add($argument)
}

$process = [Diagnostics.Process]::new()
$process.StartInfo = $startInfo
$started = $false
$stdoutTask = $null
$stderrTask = $null
$failure = $null
$jobId = $null
$resultSummary = $null
$forcedCleanup = $false

try {
    if (-not $process.Start()) {
        throw "Failed to start GigaSTT"
    }
    $started = $true
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()

    $startupClock = [Diagnostics.Stopwatch]::StartNew()
    $ready = $null
    while ($startupClock.Elapsed.TotalSeconds -lt $StartupTimeoutSeconds) {
        if ($process.HasExited) {
            throw "GigaSTT exited during startup with code $($process.ExitCode)"
        }
        try {
            $ready = Invoke-RestMethod -Uri "$BaseUrl/ready" -Method Get -TimeoutSec 3
        }
        catch {
            $ready = $null
        }
        if ($null -ne $ready -and $ready.status -eq "ready") {
            break
        }
        Start-Sleep -Milliseconds 500
    }
    if ($null -eq $ready -or $ready.status -ne "ready") {
        throw "GigaSTT did not become ready within $StartupTimeoutSeconds seconds"
    }
    if ((ConvertTo-FiniteDouble $ready.pool_available "ready.pool_available") -lt 1 -or
        (ConvertTo-FiniteDouble $ready.pool_total "ready.pool_total") -ne 1) {
        throw "Unexpected ready pool inventory"
    }

    $health = Invoke-RestMethod -Uri "$BaseUrl/health" -Method Get -TimeoutSec $RequestTimeoutSeconds
    if ($health.status -ne "ok" -or $health.version -ne "2.21.0" -or $health.variant -ne "rnnt") {
        throw "Unexpected /health identity"
    }
    if ($health.punctuation -ne $true -or $health.itn -ne $true) {
        throw "Required punctuation/ITN processors are not active"
    }

    $audioBytes = [IO.File]::ReadAllBytes($audioPath)
    $itnQuery = $RequestItn.ToString().ToLowerInvariant()
    $submitUrl = "$BaseUrl/v1/jobs?format=json&segments=true&word_timestamps=true&punctuation=true&itn=$itnQuery&vad=true"
    $submitResponse = Invoke-WebRequest -Uri $submitUrl -Method Post -ContentType "application/octet-stream" -Body $audioBytes -TimeoutSec $RequestTimeoutSeconds
    if ($submitResponse.StatusCode -ne 202) {
        throw "Expected POST /v1/jobs status 202, got $($submitResponse.StatusCode)"
    }
    $submitted = $submitResponse.Content | ConvertFrom-Json
    $jobId = [string]$submitted.job_id
    if ([string]::IsNullOrWhiteSpace($jobId) -or $submitted.status -ne "queued") {
        throw "Job submission response is invalid"
    }

    $jobClock = [Diagnostics.Stopwatch]::StartNew()
    $status = $null
    while ($jobClock.Elapsed.TotalSeconds -lt $JobTimeoutSeconds) {
        if ($process.HasExited) {
            throw "GigaSTT exited while processing job $jobId with code $($process.ExitCode)"
        }
        Start-Sleep -Milliseconds 500
        $status = Invoke-RestMethod -Uri "$BaseUrl/v1/jobs/$jobId" -Method Get -TimeoutSec $RequestTimeoutSeconds
        if ($status.status -notin @("queued", "processing", "done", "failed", "cancelled")) {
            throw "Job $jobId returned unknown status '$($status.status)'"
        }
        $percent = ConvertTo-FiniteDouble $status.percent "job.percent"
        $processedSeconds = ConvertTo-FiniteDouble $status.processed_seconds "job.processed_seconds"
        if ($percent -lt 0 -or $percent -gt 100 -or $processedSeconds -lt 0) {
            throw "Job $jobId returned invalid progress"
        }
        if ($status.status -notin @("queued", "processing")) {
            break
        }
    }
    if ($null -eq $status -or $status.status -in @("queued", "processing")) {
        try {
            Invoke-RestMethod -Uri "$BaseUrl/v1/jobs/$jobId" -Method Delete -TimeoutSec $RequestTimeoutSeconds | Out-Null
        }
        catch {
            Write-Warning "Timed-out job cancellation request failed: $($_.Exception.Message)"
        }
        throw "Job $jobId exceeded the $JobTimeoutSeconds-second deadline"
    }
    if ($status.status -ne "done") {
        throw "Job $jobId ended with status '$($status.status)'"
    }

    $result = Invoke-RestMethod -Uri "$BaseUrl/v1/jobs/$jobId/result" -Method Get -TimeoutSec $RequestTimeoutSeconds
    $text = [string]$result.text
    if (-not [string]::IsNullOrWhiteSpace($RawResultPath)) {
        # Caller keeps this outside the uploaded evidence directory. It is fed
        # into the real desktop importer to verify formatting without logging text.
        $result | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $RawResultPath -Encoding utf8NoBOM
    }
    if ([string]::IsNullOrWhiteSpace($text) -or $text -notmatch "[А-Яа-яЁё]") {
        throw "Transcription text is empty or has no Cyrillic characters"
    }
    $duration = ConvertTo-FiniteDouble $result.duration "result.duration"
    if ($duration -le 0) {
        throw "Transcription duration must be positive"
    }

    $words = @($result.words)
    if ($null -eq $result.words -or $words.Count -eq 0) {
        throw "Transcription words are missing or empty"
    }
    $previousWordStart = -1.0
    $cyrillicWordCount = 0
    foreach ($word in $words) {
        if ([string]::IsNullOrWhiteSpace([string]$word.word)) {
            throw "A result word has empty text"
        }
        if ([string]$word.word -match "[А-Яа-яЁё]") {
            $cyrillicWordCount++
        }
        $start = ConvertTo-FiniteDouble $word.start "word.start"
        $end = ConvertTo-FiniteDouble $word.end "word.end"
        $confidence = ConvertTo-FiniteDouble $word.confidence "word.confidence"
        if ($start -lt 0 -or $start -lt $previousWordStart -or $end -lt $start) {
            throw "Word timestamps are negative, reversed, or non-monotonic"
        }
        if ($confidence -lt 0 -or $confidence -gt 1) {
            throw "Word confidence is outside [0,1]"
        }
        $previousWordStart = $start
    }
    if ($cyrillicWordCount -eq 0) {
        throw "No result word contains Cyrillic text"
    }

    $segments = @($result.segments)
    if ($null -eq $result.segments -or $segments.Count -eq 0) {
        throw "Transcription segments are missing or empty"
    }
    $previousSegmentEnd = -1.0
    $segmentWordCount = 0
    foreach ($segment in $segments) {
        if ([string]::IsNullOrWhiteSpace([string]$segment.text)) {
            throw "A result segment has empty text"
        }
        $segmentStart = ConvertTo-FiniteDouble $segment.start "segment.start"
        $segmentEnd = ConvertTo-FiniteDouble $segment.end "segment.end"
        if ($segmentStart -lt $previousSegmentEnd -or $segmentEnd -lt $segmentStart) {
            throw "Segment timestamps overlap, reverse, or are non-monotonic"
        }
        $segmentWords = @($segment.words)
        if ($null -eq $segment.words -or $segmentWords.Count -eq 0) {
            throw "A result segment has no words"
        }
        $segmentWordCount += $segmentWords.Count
        $previousSegmentEnd = $segmentEnd
    }
    if ($segmentWordCount -ne $words.Count) {
        throw "Segment words ($segmentWordCount) do not cover the top-level words ($($words.Count))"
    }

    $resultSummary = [ordered]@{
        schemaVersion = 1
        generatedAtUtc = [DateTime]::UtcNow.ToString("o")
        gigasttVersion = [string]$health.version
        variant = [string]$health.variant
        fixtureSha256 = $actualAudioSha
        jobId = $jobId
        durationSeconds = $duration
        wordCount = $words.Count
        segmentCount = $segments.Count
        cyrillicWordCount = $cyrillicWordCount
        textCharacters = $text.Length
        textSha256 = Get-StringSha256 -Value $text
        startupElapsedSeconds = [Math]::Round($startupClock.Elapsed.TotalSeconds, 3)
        jobElapsedSeconds = [Math]::Round($jobClock.Elapsed.TotalSeconds, 3)
        punctuation = [bool]$health.punctuation
        requestItn = $RequestItn
        itn = [bool]$health.itn
        shutdown = "pending"
    }
}
catch {
    $failure = $_.Exception.Message
}
finally {
    if ($started -and -not $process.HasExited) {
        try {
            # The pinned server only exposes console CTRL_C as a graceful
            # Windows shutdown trigger. Broadcasting CTRL_C_EVENT from a hosted
            # Actions shell would also signal unrelated processes sharing that
            # console. This native build/inference smoke therefore reaps the
            # child forcibly and records that limitation; it does not claim the
            # separate graceful-shutdown release acceptance gate.
            $forcedCleanup = $true
            $process.Kill($true)
            if (-not $process.WaitForExit($ShutdownTimeoutSeconds * 1000)) {
                throw "GigaSTT was not reaped within $ShutdownTimeoutSeconds seconds after Kill(true)"
            }
        }
        catch {
            $failure = Add-Failure -Current $failure -Next $_.Exception.Message
        }
    }
    elseif ($started -and [string]::IsNullOrWhiteSpace($failure)) {
        $failure = "GigaSTT exited before the harness requested shutdown"
    }

    if ($started -and $process.HasExited -and $null -ne $stdoutTask -and $stdoutTask.Wait(5000)) {
        [IO.File]::WriteAllText($stdoutPath, $stdoutTask.Result, [Text.UTF8Encoding]::new($false))
    }
    elseif ($started -and $null -ne $stdoutTask) {
        $failure = Add-Failure -Current $failure -Next "stdout drain did not finish within 5 seconds"
    }
    if ($started -and $process.HasExited -and $null -ne $stderrTask -and $stderrTask.Wait(5000)) {
        [IO.File]::WriteAllText($stderrPath, $stderrTask.Result, [Text.UTF8Encoding]::new($false))
    }
    elseif ($started -and $null -ne $stderrTask) {
        $failure = Add-Failure -Current $failure -Next "stderr drain did not finish within 5 seconds"
    }
    $process.Dispose()
}

$combinedLog = ""
if (Test-Path -LiteralPath $stdoutPath) {
    $combinedLog += Get-Content -LiteralPath $stdoutPath -Raw
}
if (Test-Path -LiteralPath $stderrPath) {
    $combinedLog += Get-Content -LiteralPath $stderrPath -Raw
}
if ($combinedLog -notmatch "VAD enabled \(model dir:") {
    $failure = Add-Failure -Current $failure -Next "Server log does not prove that the required VAD model loaded"
}
if ($null -ne $resultSummary) {
    $resultSummary.shutdown = if ([string]::IsNullOrWhiteSpace($failure) -and $forcedCleanup) { "forced-reaped" } else { "failed" }
    $resultSummary | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $summaryPath -Encoding utf8NoBOM
}

if (-not [string]::IsNullOrWhiteSpace($failure)) {
    throw "GigaSTT Windows smoke failed: $failure. Logs: $EvidenceDir"
}

Write-Host ("NATIVE INFERENCE SMOKE PASS job={0} duration={1} words={2} segments={3} cyrillic_words={4} shutdown=forced-reaped graceful_shutdown=NOT_VERIFIED" -f `
    $jobId,
    $resultSummary.durationSeconds,
    $resultSummary.wordCount,
    $resultSummary.segmentCount,
    $resultSummary.cyrillicWordCount)
