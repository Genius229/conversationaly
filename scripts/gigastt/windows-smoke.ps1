[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$Binary,
    [Parameter(Mandatory=$true)][string]$Audio,
    [string]$BaseUrl = "http://127.0.0.1:9876",
    [int]$StartupTimeoutSeconds = 120
)

$ErrorActionPreference = "Stop"
$proc = $null
try {
    $proc = Start-Process -FilePath $Binary -ArgumentList @("serve", "--host", "127.0.0.1", "--enable-jobs", "--pool-size", "1", "--batch-pool-size", "1", "--vad", "--punctuation", "--itn") -PassThru -RedirectStandardOutput "$env:TEMP\gigastt.stdout.log" -RedirectStandardError "$env:TEMP\gigastt.stderr.log"
    $deadline = (Get-Date).AddSeconds($StartupTimeoutSeconds)
    do {
        Start-Sleep -Milliseconds 500
        try { $ready = Invoke-RestMethod "$BaseUrl/ready" -TimeoutSec 2 } catch { $ready = $null }
    } while (-not $ready -and (Get-Date) -lt $deadline)
    if (-not $ready -or $ready.status -ne "ready") { throw "GigaSTT did not become ready" }

    $bytes = [IO.File]::ReadAllBytes((Resolve-Path $Audio))
    $response = Invoke-WebRequest "$BaseUrl/v1/jobs?format=json&segments=true&word_timestamps=true&punctuation=true&itn=true&vad=true" -Method Post -ContentType "application/octet-stream" -Body $bytes
    if ($response.StatusCode -ne 202) { throw "Expected 202, got $($response.StatusCode)" }
    $job = $response.Content | ConvertFrom-Json
    if ([string]::IsNullOrWhiteSpace($job.job_id)) { throw "Response has no job_id" }

    do {
        Start-Sleep -Seconds 1
        $status = Invoke-RestMethod "$BaseUrl/v1/jobs/$($job.job_id)"
    } while ($status.status -in @("queued", "processing"))
    if ($status.status -ne "done") { throw "Job ended with status $($status.status)" }
    $result = Invoke-RestMethod "$BaseUrl/v1/jobs/$($job.job_id)/result"
    if ([string]::IsNullOrWhiteSpace($result.text)) { throw "Empty transcription result" }
    if (-not $result.duration) { throw "Result has no duration" }
    Write-Output ("SMOKE PASS job={0} duration={1} text_chars={2}" -f $job.job_id, $result.duration, $result.text.Length)
}
finally {
    if ($proc) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
}
