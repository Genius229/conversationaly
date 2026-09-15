[CmdletBinding()]
param(
    [string]$ReportPath,
    [string]$FfmpegPath
)

$ErrorActionPreference = 'Stop'
$script:Results = @()
$script:StartedAt = [DateTime]::UtcNow
$script:CurrentSkipReason = $null
$script:TestRoot = Join-Path ([IO.Path]::GetTempPath()) ('capture-diagnostics-tests-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $script:TestRoot -Force | Out-Null

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

function Assert-Equal {
    param($Expected, $Actual, [string]$Message)
    if ($Expected -ne $Actual) {
        throw ("{0}: expected <{1}>, got <{2}>" -f $Message, $Expected, $Actual)
    }
}

function Assert-SequenceEqual {
    param([object[]]$Expected, [object[]]$Actual, [string]$Message)
    Assert-Equal $Expected.Count $Actual.Count ($Message + ' length')
    for ($i = 0; $i -lt $Expected.Count; $i++) {
        Assert-Equal $Expected[$i] $Actual[$i] ("{0} item {1}" -f $Message, $i)
    }
}

function Assert-ThrowsLike {
    param([scriptblock]$Action, [string]$Pattern, [string]$Message)
    $caught = $null
    try { & $Action } catch { $caught = $_ }
    if ($null -eq $caught) { throw ($Message + ': expected an exception') }
    if ($caught.Exception.Message -notlike $Pattern) {
        throw ("{0}: wrong exception <{1}>" -f $Message, $caught.Exception.Message)
    }
}

function Invoke-Test {
    param([string]$Name, [scriptblock]$Body)
    $start = [Diagnostics.Stopwatch]::StartNew()
    $script:CurrentSkipReason = $null
    try {
        & $Body
        if ($null -ne $script:CurrentSkipReason) {
            $script:Results += [pscustomobject]@{ name = $Name; status = 'skip'; passed = $null; milliseconds = $start.ElapsedMilliseconds; error = $null; skipReason = $script:CurrentSkipReason }
            Write-Host ("SKIP {0}: {1}" -f $Name, $script:CurrentSkipReason) -ForegroundColor Yellow
        } else {
            $script:Results += [pscustomobject]@{ name = $Name; status = 'pass'; passed = $true; milliseconds = $start.ElapsedMilliseconds; error = $null; skipReason = $null }
            Write-Host ("PASS {0}" -f $Name)
        }
    } catch {
        $script:Results += [pscustomobject]@{ name = $Name; status = 'fail'; passed = $false; milliseconds = $start.ElapsedMilliseconds; error = $_.Exception.ToString(); skipReason = $null }
        Write-Host ("FAIL {0}: {1}" -f $Name, $_.Exception.Message) -ForegroundColor Red
    }
}

function Set-TestSkipped {
    param([Parameter(Mandatory = $true)][string]$Reason)
    $script:CurrentSkipReason = $Reason
}

function New-FakeScript {
    param([string]$Name, [string]$Body)
    $path = Join-Path $script:TestRoot $Name
    [IO.File]::WriteAllText($path, $Body, (New-Object Text.UTF8Encoding($true)))
    return $path
}

function Get-TestHostPath {
    return [Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
}

function Get-TestHostPrefix {
    param([string]$ScriptPath)
    return @('-NoLogo', '-NoProfile', '-NonInteractive', '-File', $ScriptPath)
}

function Convert-StrictUtf8ForTest {
    param([byte[]]$Bytes)
    return (New-Object Text.UTF8Encoding($false, $true)).GetString($Bytes)
}

$collector = Join-Path $PSScriptRoot 'collect-dshow.ps1'
. $collector

Invoke-Test 'production argument arrays stay exact' {
    Assert-SequenceEqual @('-hide_banner','-nostats','-nostdin','-list_devices','true','-f','dshow','-i','dummy') @(Get-DShowEnumerationArguments) 'enumeration argv'
    Assert-SequenceEqual @('-hide_banner','-nostats','-nostdin','-loglevel','debug','-list_options','true','-f','dshow','-i','audio=@device_cm_{A}\wave_{B}') @(Get-DShowOptionsArguments -Token '@device_cm_{A}\wave_{B}') 'options argv'
    Assert-SequenceEqual @('-hide_banner','-nostats','-loglevel','error','-f','dshow','-i','audio=Микрофон "Desk" \ rear','-map','0:a:0','-ac','1','-ar','48000','-c:a','pcm_f32le','-f','f32le','pipe:1') @(Get-DShowCaptureArguments -Token 'Микрофон "Desk" \ rear') 'capture argv'
}

Invoke-Test 'parser pairs audio friendly and alternative records from the right' {
    $text = @'
[dshow @ 000000000001] "Camera One" (video)
[dshow @ 000000000001]   Alternative name "@device_pnp_camera"
[dshow @ 000000000001] "Микрофон "Desk" \ rear" (audio)
[dshow @ 000000000001]   Alternative name "@device_cm_{A}\wave_{B}"
[dshow @ 000000000001] "Combo input" (video, audio)
[dshow @ 000000000001]   Alternative name "@device_cm_{C}\wave_{D}"
'@
    $bytes = (New-Object Text.UTF8Encoding($false)).GetBytes($text)
    $devices = @(ConvertFrom-DShowEnumerationBytes -Bytes $bytes)
    Assert-Equal 2 $devices.Count 'only audio-capable records'
    Assert-Equal 'Микрофон "Desk" \ rear' $devices[0].FriendlyName 'embedded quote/backslash friendly name'
    Assert-Equal '@device_cm_{A}\wave_{B}' $devices[0].Moniker 'paired moniker'
    Assert-Equal 'Combo input' $devices[1].FriendlyName 'mixed record remains audio-capable'
}

Invoke-Test 'parser fails closed for invalid UTF-8' {
    Assert-ThrowsLike { ConvertFrom-DShowEnumerationBytes -Bytes ([byte[]](0xFF, 0xFE, 0xFF)) } '*UTF-8*' 'invalid bytes'
}

Invoke-Test 'parser fails closed for incomplete or crossed pairs' {
    $text = "[dshow @ one] `"Mic`" (audio)`n[dshow @ two] Alternative name `"@device_cm_x`"`n"
    $bytes = [Text.Encoding]::UTF8.GetBytes($text)
    Assert-ThrowsLike { ConvertFrom-DShowEnumerationBytes -Bytes $bytes } '*malformed*' 'context mismatch'
}

Invoke-Test 'selection rejects missing video-only and duplicate friendly names' {
    $devices = @(
        [pscustomobject]@{ FriendlyName = 'Camera'; Moniker = '@camera'; Audio = $false },
        [pscustomobject]@{ FriendlyName = 'Same'; Moniker = '@mic-one'; Audio = $true },
        [pscustomobject]@{ FriendlyName = 'Same'; Moniker = '@mic-two'; Audio = $true }
    )
    Assert-ThrowsLike { Select-DShowDevice -Devices $devices -ExactName 'Camera' } '*not found*' 'video-only is not selectable'
    Assert-ThrowsLike { Select-DShowDevice -Devices $devices -ExactName 'Missing' } '*not found*' 'missing exact name'
    Assert-ThrowsLike { Select-DShowDevice -Devices $devices -ExactName 'Same' } '*ambiguous*' 'duplicate exact name'
    Assert-ThrowsLike { Select-DShowDevice -Devices $devices -ExactName 'same' } '*not found*' 'ordinal case-sensitive match'
}

Invoke-Test 'install path normalization removes only surrounding quotes' {
    Assert-Equal 'C:\Users\Helper Name\AppData\Local\Conversationaly GigaSTT Dev' (Normalize-InstallLocation -Value '  "C:\Users\Helper Name\AppData\Local\Conversationaly GigaSTT Dev"  ') 'quoted install path'
    Assert-Equal 'C:\plain\path' (Normalize-InstallLocation -Value ' C:\plain\path ') 'plain install path'
    Assert-ThrowsLike { Normalize-InstallLocation -Value '"broken' } '*malformed*' 'one-sided quote'
}

Invoke-Test 'diagnostic sequence probes both identities after natural moniker failure' {
    $enumText = "[dshow @ ctx] `"Exact Mic`" (audio)`n[dshow @ ctx] Alternative name `"@device_cm_{ONE}\wave_{TWO}`"`n"
    $enumBytes = [Text.Encoding]::UTF8.GetBytes($enumText)
    $calls = New-Object Collections.ArrayList
    $progress = New-Object Collections.ArrayList
    $fakeInvoker = {
        param([string]$Kind, [string]$FilePath, [string[]]$Arguments, [bool]$RetainStdout)
        [void]$calls.Add([pscustomobject]@{ Kind = $Kind; FilePath = $FilePath; Arguments = @($Arguments); RetainStdout = $RetainStdout })
        $stdout = [byte[]]@()
        $stderr = [byte[]]@()
        $exit = 0
        $outcome = 'natural_exit'
        if ($Kind -eq 'version') { $stdout = [Text.Encoding]::UTF8.GetBytes('ffmpeg test build') }
        if ($Kind -eq 'enumeration') { $stderr = $enumBytes; $exit = 1 }
        if ($Kind -eq 'options-moniker') { $stderr = [Text.Encoding]::UTF8.GetBytes('native moniker options'); $exit = 1 }
        if ($Kind -eq 'options-friendly') { $stderr = [Text.Encoding]::UTF8.GetBytes('native friendly options'); $exit = 1 }
        if ($Kind -eq 'capture-moniker') { $stderr = [Text.Encoding]::UTF8.GetBytes('0x80070057'); $exit = 1; $outcome = 'natural_exit_before_data' }
        if ($Kind -eq 'capture-friendly') { $stderr = [Text.Encoding]::UTF8.GetBytes('friendly started'); $exit = 0; $outcome = 'planned_stop_after_data' }
        [pscustomobject]@{
            Started = $true; Arguments = @($Arguments); StartedAtUtc = [DateTime]::UtcNow; FinishedAtUtc = [DateTime]::UtcNow
            DurationMilliseconds = 1; FirstStdoutByteMilliseconds = $(if ($Kind -eq 'capture-friendly') { 1 } else { $null })
            ExitCode = $exit; ForcedStop = $false; StopRequested = $Kind -like 'capture-*'; TimedOut = $false
            TotalStdoutBytes = $(if ($Kind -eq 'capture-friendly') { 4096 } else { $stdout.Length }); TotalStderrBytes = $stderr.Length
            RetainedStdout = $stdout; RetainedStderr = $stderr; StdoutTruncated = $false; StderrTruncated = $false
            CleanupComplete = $true; CleanupError = $null; JobAssigned = $true; Outcome = $outcome; LaunchError = $null
        }
    }
    $progressSink = { param([string]$Message) [void]$progress.Add($Message) }
    $sequence = Invoke-DiagnosticSequence -FfmpegPath 'C:\fake\ffmpeg.exe' -SelectedName 'Exact Mic' -ProbeInvoker $fakeInvoker -ProgressSink $progressSink
    Assert-Equal 'Exact Mic' $sequence.Selection.FriendlyName 'selected friendly name'
    Assert-Equal '@device_cm_{ONE}\wave_{TWO}' $sequence.Selection.Moniker 'selected exact moniker'
    Assert-Equal 6 $calls.Count 'version enumeration two options two captures'
    Assert-Equal 'capture-friendly' $calls[5].Kind 'friendly capture still ran after moniker failure'
    Assert-SequenceEqual @(Get-DShowCaptureArguments -Token '@device_cm_{ONE}\wave_{TWO}') @($calls[4].Arguments) 'moniker production capture argv'
    Assert-SequenceEqual @(Get-DShowCaptureArguments -Token 'Exact Mic') @($calls[5].Arguments) 'friendly production capture argv'
    Assert-True (@($progress | Where-Object { $_ -like '*Выбран микрофон*Exact Mic*' }).Count -eq 1) 'selection progress shown in Russian'
    Assert-True (@($progress | Where-Object { $_ -like '*служебного имени*до * секунд*' }).Count -ge 1) 'bounded moniker capture progress shown'
    Assert-True (@($progress | Where-Object { $_ -like '*видимого имени*до * секунд*' }).Count -ge 1) 'bounded friendly capture progress shown'
}

Invoke-Test 'diagnostic sequence refuses truncated enumeration before selection' {
    $calls = New-Object Collections.ArrayList
    $fakeInvoker = {
        param([string]$Kind, [string]$FilePath, [string[]]$Arguments, [bool]$RetainStdout)
        [void]$calls.Add($Kind)
        $stderr = [byte[]]@()
        $truncated = $false
        $totalStderr = 0
        $stdout = [byte[]]@()
        if ($Kind -eq 'version') { $stdout = [Text.Encoding]::UTF8.GetBytes('ffmpeg test') }
        if ($Kind -eq 'enumeration') {
            $stderr = [Text.Encoding]::UTF8.GetBytes("[dshow @ x] `"Mic`" (audio)`n[dshow @ x] Alternative name `"@device_cm_x`"`n")
            $truncated = $true
            $totalStderr = 131073
        }
        [pscustomobject]@{
            Started = $true; Arguments = @($Arguments); StartedAtUtc = [DateTime]::UtcNow; FinishedAtUtc = [DateTime]::UtcNow
            DurationMilliseconds = 1; FirstStdoutByteMilliseconds = $null; ExitCode = 0; ForcedStop = $false
            StopRequested = $false; TimedOut = $false; TotalStdoutBytes = $stdout.Length; TotalStderrBytes = $totalStderr
            RetainedStdout = $stdout; RetainedStderr = $stderr; StdoutTruncated = $false; StderrTruncated = $truncated
            CleanupComplete = $true; CleanupError = $null; StopWriteError = $null; JobAssigned = $true
            Outcome = 'natural_exit'; LaunchError = $null
        }
    }
    $sequence = Invoke-DiagnosticSequence -FfmpegPath 'C:\fake\ffmpeg.exe' -SelectedName 'Mic' -ProbeInvoker $fakeInvoker
    Assert-True ($sequence.Error -like '*exceeded*') 'truncation is an identity error'
    Assert-Equal 2 $calls.Count 'no identity probes after truncated enumeration'
}

Invoke-Test 'diagnostic sequence refuses incomplete enumeration drain' {
    $enumBytes = [Text.Encoding]::UTF8.GetBytes("[dshow @ x] `"Mic`" (audio)`n[dshow @ x] Alternative name `"@device_cm_x`"`n")
    $calls = New-Object Collections.ArrayList
    $fakeInvoker = {
        param([string]$Kind, [string]$FilePath, [string[]]$Arguments, [bool]$RetainStdout)
        [void]$calls.Add($Kind)
        $stderr = $(if ($Kind -eq 'enumeration') { $enumBytes } else { [byte[]]@() })
        $stdout = $(if ($Kind -eq 'version') { [Text.Encoding]::UTF8.GetBytes('ffmpeg test') } else { [byte[]]@() })
        [pscustomobject]@{
            Started = $true; Arguments = @($Arguments); StartedAtUtc = [DateTime]::UtcNow; FinishedAtUtc = [DateTime]::UtcNow
            DurationMilliseconds = 1; FirstStdoutByteMilliseconds = $null; ExitCode = 0; ForcedStop = $false
            StopRequested = $false; TimedOut = $false; TotalStdoutBytes = $stdout.Length; TotalStderrBytes = $stderr.Length
            RetainedStdout = $stdout; RetainedStderr = $stderr; StdoutTruncated = $false; StderrTruncated = $false
            CleanupComplete = $Kind -ne 'enumeration'; CleanupError = $(if ($Kind -eq 'enumeration') { 'stderr drain failed' } else { $null })
            StopWriteError = $null; JobAssigned = $true; Outcome = 'natural_exit'; LaunchError = $null
        }
    }
    $sequence = Invoke-DiagnosticSequence -FfmpegPath 'C:\fake\ffmpeg.exe' -SelectedName 'Mic' -ProbeInvoker $fakeInvoker
    Assert-True ($sequence.Error -like '*cleanup*') 'incomplete drain is an identity error'
    Assert-Equal 2 $calls.Count 'no selectors run from partial enumeration bytes'
}

Invoke-Test 'diagnostic sequence refuses timed-out or forced enumeration' {
    $enumBytes = [Text.Encoding]::UTF8.GetBytes("[dshow @ x] `"Mic`" (audio)`n[dshow @ x] Alternative name `"@device_cm_x`"`n")
    $modes = @(
        [pscustomobject]@{ Name = 'timeout'; TimedOut = $true; ForcedStop = $false },
        [pscustomobject]@{ Name = 'forced stop'; TimedOut = $false; ForcedStop = $true }
    )
    foreach ($mode in $modes) {
        $calls = New-Object Collections.ArrayList
        $fakeInvoker = {
            param([string]$Kind, [string]$FilePath, [string[]]$Arguments, [bool]$RetainStdout)
            [void]$calls.Add($Kind)
            $stderr = $(if ($Kind -eq 'enumeration') { $enumBytes } else { [byte[]]@() })
            $stdout = $(if ($Kind -eq 'version') { [Text.Encoding]::UTF8.GetBytes('ffmpeg test') } else { [byte[]]@() })
            [pscustomobject]@{
                Started = $true; Arguments = @($Arguments); StartedAtUtc = [DateTime]::UtcNow; FinishedAtUtc = [DateTime]::UtcNow
                DurationMilliseconds = 1; FirstStdoutByteMilliseconds = $null; ExitCode = 0
                ForcedStop = $Kind -eq 'enumeration' -and $mode.ForcedStop
                StopRequested = $false; TimedOut = $Kind -eq 'enumeration' -and $mode.TimedOut
                TotalStdoutBytes = $stdout.Length; TotalStderrBytes = $stderr.Length
                RetainedStdout = $stdout; RetainedStderr = $stderr; StdoutTruncated = $false; StderrTruncated = $false
                CleanupComplete = $true; CleanupError = $null; StopWriteError = $null; JobAssigned = $true
                Outcome = 'natural_exit'; LaunchError = $null
            }
        }
        $sequence = Invoke-DiagnosticSequence -FfmpegPath 'C:\fake\ffmpeg.exe' -SelectedName 'Mic' -ProbeInvoker $fakeInvoker
        Assert-True ($sequence.Error -like '*incomplete*') ($mode.Name + ' is an identity error')
        Assert-Equal 2 $calls.Count ('no selectors run after ' + $mode.Name)
    }
}

Invoke-Test 'colon in friendly name never reaches FFmpeg selector grammar' {
    $unsafeName = 'Mic:audio=Other'
    $enumText = "[dshow @ x] `"$unsafeName`" (audio)`n[dshow @ x] Alternative name `"@device_cm_safe`"`n"
    $enumBytes = [Text.Encoding]::UTF8.GetBytes($enumText)
    $calls = New-Object Collections.ArrayList
    $fakeInvoker = {
        param([string]$Kind, [string]$FilePath, [string[]]$Arguments, [bool]$RetainStdout)
        [void]$calls.Add([pscustomobject]@{ Kind = $Kind; Arguments = @($Arguments) })
        $stderr = $(if ($Kind -eq 'enumeration') { $enumBytes } else { [byte[]]@() })
        $stdout = $(if ($Kind -eq 'version') { [Text.Encoding]::UTF8.GetBytes('ffmpeg test') } else { [byte[]]@() })
        [pscustomobject]@{
            Started = $true; Arguments = @($Arguments); StartedAtUtc = [DateTime]::UtcNow; FinishedAtUtc = [DateTime]::UtcNow
            DurationMilliseconds = 1; FirstStdoutByteMilliseconds = $null; ExitCode = 0; ForcedStop = $false
            StopRequested = $false; TimedOut = $false; TotalStdoutBytes = $stdout.Length; TotalStderrBytes = $stderr.Length
            RetainedStdout = $stdout; RetainedStderr = $stderr; StdoutTruncated = $false; StderrTruncated = $false
            CleanupComplete = $true; CleanupError = $null; StopWriteError = $null; JobAssigned = $true
            Outcome = 'natural_exit'; LaunchError = $null
        }
    }
    $sequence = Invoke-DiagnosticSequence -FfmpegPath 'C:\fake\ffmpeg.exe' -SelectedName $unsafeName -ProbeInvoker $fakeInvoker
    Assert-Equal 4 $calls.Count 'only version enumeration and two safe moniker probes launch'
    foreach ($call in $calls) {
        Assert-True (-not (@($call.Arguments) -contains ('audio=' + $unsafeName))) 'unsafe friendly selector never launched'
    }
    $friendlyEntries = @($sequence.Processes | Where-Object { $_.Kind -in @('options-friendly','capture-friendly') })
    Assert-Equal 2 $friendlyEntries.Count 'both unsafe friendly probes recorded'
    foreach ($entry in $friendlyEntries) {
        Assert-Equal 'skipped_unsafe_selector' $entry.Result.Outcome 'unsafe probe outcome'
        Assert-True ($entry.Result.SkipReason -like '*colon*') 'unsafe reason recorded'
    }
}

Invoke-Test 'archive preserves partial JSON without audio artifacts' {
    $out = Join-Path $script:TestRoot 'archive parent with spaces'
    New-Item -ItemType Directory -Path $out -Force | Out-Null
    $archive = Write-DiagnosticArchive -OutputParent $out -Report ([ordered]@{ status = 'partial'; error = 'selection cancelled'; processes = @() })
    Assert-True (Test-Path -LiteralPath $archive.ReportPath -PathType Leaf) 'partial report JSON exists'
    Assert-True (Test-Path -LiteralPath $archive.ZipPath -PathType Leaf) 'partial ZIP exists'
    $json = Get-Content -LiteralPath $archive.ReportPath -Raw | ConvertFrom-Json
    Assert-Equal 'partial' $json.status 'partial status preserved'
    $audio = @(Get-ChildItem -LiteralPath $archive.DirectoryPath -Recurse -File | Where-Object { $_.Extension -in @('.wav','.pcm','.raw','.f32le','.mp3','.ogg','.m4a') })
    Assert-Equal 0 $audio.Count 'no audio-like file stored'
}

Invoke-Test 'output location falls back after unwritable candidate' {
    $notDirectory = Join-Path $script:TestRoot 'blocked-parent'
    [IO.File]::WriteAllText($notDirectory, 'file blocks directory')
    $fallback = Join-Path $script:TestRoot 'writable fallback'
    $selected = Get-DiagnosticOutputParent -Candidates @($notDirectory, $fallback)
    Assert-Equal $fallback $selected 'first writable candidate selected'
    Assert-Equal 0 @(Get-ChildItem -LiteralPath $fallback -Force).Count 'writability probe cleaned up'
}

Invoke-Test 'compression failure returns surviving report directory' {
    $out = Join-Path $script:TestRoot 'compression failure'
    $failCompression = { param([string]$SourcePattern, [string]$DestinationPath) throw 'compression blocked for test' }
    $archive = Write-DiagnosticArchive -OutputParent $out -Report ([ordered]@{ status = 'partial'; error = 'probe failed'; processes = @() }) -CompressionInvoker $failCompression
    Assert-True (Test-Path -LiteralPath $archive.ReportPath -PathType Leaf) 'report survives compression failure'
    Assert-True (Test-Path -LiteralPath $archive.DirectoryPath -PathType Container) 'report directory survives compression failure'
    Assert-True ([string]::IsNullOrWhiteSpace([string]$archive.ZipPath)) 'failed ZIP is not claimed'
    Assert-True ($archive.CompressionError -like '*compression blocked*') 'compression error returned'
}

Invoke-Test 'runner preserves Cyrillic spaces quotes and trailing backslashes as argv' {
    $fake = New-FakeScript 'argv.ps1' @'
$json = ConvertTo-Json -Compress -InputObject @($args)
$bytes = (New-Object Text.UTF8Encoding($false)).GetBytes($json)
[Console]::OpenStandardOutput().Write($bytes, 0, $bytes.Length)
'@
    $special = @('Микрофон USB', 'a"b', 'C:\path with space\', '', '\\server\share\')
    $all = @(Get-TestHostPrefix $fake) + $special
    $result = Invoke-OwnedProcess -FilePath (Get-TestHostPath) -Arguments $all -TimeoutMilliseconds 5000 -RetainStdout
    Assert-Equal 0 $result.ExitCode 'argv child exit'
    $decoded = Convert-StrictUtf8ForTest $result.RetainedStdout
    # Windows PowerShell 5.1 emits a JSON array from ConvertFrom-Json as one
    # pipeline object. Assign first, then normalize the actual array value.
    $parsedRoundTrip = ConvertFrom-Json $decoded
    $roundTrip = [object[]]$parsedRoundTrip
    Assert-SequenceEqual $special $roundTrip 'argv round trip'
}

Invoke-Test 'runner drains bounded stderr flood without deadlock' {
    $fake = New-FakeScript 'stderr-flood.ps1' @'
$chunk = [byte[]]::new(4096)
for ($i = 0; $i -lt $chunk.Length; $i++) { $chunk[$i] = 88 }
$stream = [Console]::OpenStandardError()
for ($i = 0; $i -lt 49; $i++) { $stream.Write($chunk, 0, $chunk.Length) }
$tail = [byte[]]::new(3392)
for ($i = 0; $i -lt $tail.Length; $i++) { $tail[$i] = 89 }
$stream.Write($tail, 0, $tail.Length)
'@
    $result = Invoke-OwnedProcess -FilePath (Get-TestHostPath) -Arguments (Get-TestHostPrefix $fake) -TimeoutMilliseconds 5000
    Assert-Equal 204096 $result.TotalStderrBytes 'all stderr drained'
    Assert-Equal 131072 $result.RetainedStderr.Length 'stderr retained cap'
    Assert-True $result.StderrTruncated 'stderr truncation marked'
    Assert-Equal 0 $result.ExitCode 'flood child exit'
}

Invoke-Test 'runner reports launch failure with finite cleanup' {
    $missing = Join-Path $script:TestRoot 'definitely-missing-executable.exe'
    $start = [Diagnostics.Stopwatch]::StartNew()
    $result = Invoke-OwnedProcess -FilePath $missing -Arguments @('--never-runs') -TimeoutMilliseconds 500
    Assert-True (-not $result.Started) 'missing executable never started'
    Assert-Equal 'launch_failure' $result.Outcome 'launch failure outcome'
    Assert-True (-not [string]::IsNullOrWhiteSpace($result.LaunchError)) 'launch error retained'
    Assert-True $result.CleanupComplete 'failed launch cleaned owned resources'
    Assert-True ($start.ElapsedMilliseconds -lt 3000) 'launch failure remained bounded'
}

Invoke-Test 'capture runner discards binary stdout while counting bytes' {
    $fake = New-FakeScript 'binary.ps1' @'
$bytes = [byte[]](0, 255, 1, 254, 2, 253)
[Console]::OpenStandardOutput().Write($bytes, 0, $bytes.Length)
'@
    $result = Invoke-OwnedCaptureProcess -FilePath (Get-TestHostPath) -Arguments (Get-TestHostPrefix $fake) -FirstDataTimeoutMilliseconds 1500 -CaptureAfterFirstDataMilliseconds 100 -QuitGraceMilliseconds 500 -KillGraceMilliseconds 1500
    Assert-Equal 6 $result.TotalStdoutBytes 'binary count'
    Assert-Equal 0 $result.RetainedStdout.Length 'capture bytes discarded'
    Assert-True ($null -ne $result.FirstStdoutByteMilliseconds) 'first-byte timing retained'
    Assert-Equal 0 $result.ExitCode 'binary child exit'
}

Invoke-Test 'capture runner reports natural exit before data' {
    $fake = New-FakeScript 'natural-failure.ps1' 'exit 23'
    $result = Invoke-OwnedCaptureProcess -FilePath (Get-TestHostPath) -Arguments (Get-TestHostPrefix $fake) -FirstDataTimeoutMilliseconds 1200 -CaptureAfterFirstDataMilliseconds 100 -QuitGraceMilliseconds 300 -KillGraceMilliseconds 1000
    Assert-Equal 23 $result.ExitCode 'natural failure exit code'
    Assert-Equal 'natural_exit_before_data' $result.Outcome 'natural failure outcome'
    Assert-True (-not $result.ForcedStop) 'natural exit was not killed'
}

Invoke-Test 'capture runner stops startup stall with q' {
    $fake = New-FakeScript 'startup-stall.ps1' @'
$line = [Console]::In.ReadLine()
if ($line -eq 'q') { exit 0 }
exit 9
'@
    $result = Invoke-OwnedCaptureProcess -FilePath (Get-TestHostPath) -Arguments (Get-TestHostPrefix $fake) -FirstDataTimeoutMilliseconds 300 -CaptureAfterFirstDataMilliseconds 100 -QuitGraceMilliseconds 1200 -KillGraceMilliseconds 1000
    Assert-Equal 'no_data_timeout' $result.Outcome 'stall outcome'
    Assert-True $result.StopRequested 'q requested after first-data timeout'
    Assert-True (-not $result.ForcedStop) 'q stopped stalled child'
    Assert-Equal 0 $result.ExitCode 'stalled child handled q'
}

Invoke-Test 'capture runner sends q after bounded data window' {
    $fake = New-FakeScript 'q-stop.ps1' @'
$bytes = [byte[]](1, 2, 3, 4)
[Console]::OpenStandardOutput().Write($bytes, 0, $bytes.Length)
[Console]::OpenStandardOutput().Flush()
$line = [Console]::In.ReadLine()
if ($line -eq 'q') { exit 0 }
exit 8
'@
    $result = Invoke-OwnedCaptureProcess -FilePath (Get-TestHostPath) -Arguments (Get-TestHostPrefix $fake) -FirstDataTimeoutMilliseconds 1500 -CaptureAfterFirstDataMilliseconds 200 -QuitGraceMilliseconds 1200 -KillGraceMilliseconds 1000
    Assert-Equal 'planned_stop_after_data' $result.Outcome 'planned stop outcome'
    Assert-True $result.StopRequested 'q sent'
    Assert-True (-not $result.ForcedStop) 'q avoided kill'
    Assert-Equal 0 $result.ExitCode 'q child exit'
}

Invoke-Test 'capture runner force-stops owned process and descendants' {
    if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
        Set-TestSkipped 'Windows Job Object behavior is Windows-only'
        return
    }
    $fake = New-FakeScript 'forced-stop.ps1' @'
$hostPath = [Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
$child = Start-Process -FilePath $hostPath -ArgumentList @('-NoLogo','-NoProfile','-NonInteractive','-Command','Start-Sleep -Seconds 30') -PassThru -WindowStyle Hidden
[Console]::Error.WriteLine(('CHILD_PID=' + $child.Id))
$bytes = [byte[]](7)
[Console]::OpenStandardOutput().Write($bytes, 0, 1)
[Console]::OpenStandardOutput().Flush()
Start-Sleep -Seconds 30
'@
    $result = Invoke-OwnedCaptureProcess -FilePath (Get-TestHostPath) -Arguments (Get-TestHostPrefix $fake) -FirstDataTimeoutMilliseconds 1500 -CaptureAfterFirstDataMilliseconds 100 -QuitGraceMilliseconds 200 -KillGraceMilliseconds 1500
    Assert-True $result.ForcedStop 'uncooperative owner killed'
    Assert-Equal 'planned_stop_after_data' $result.Outcome 'forced planned outcome'
    $stderr = Convert-StrictUtf8ForTest $result.RetainedStderr
    $match = [regex]::Match($stderr, 'CHILD_PID=(\d+)')
    Assert-True $match.Success 'child pid captured'
    Start-Sleep -Milliseconds 200
    Assert-True ($null -eq (Get-Process -Id ([int]$match.Groups[1].Value) -ErrorAction SilentlyContinue)) 'job close killed descendant'
    Assert-True $result.CleanupComplete 'owned cleanup completed'
}

Invoke-Test 'real ffmpeg lavfi smoke produces expected discarded byte count' {
    if ([string]::IsNullOrWhiteSpace($FfmpegPath)) {
        Set-TestSkipped 'No -FfmpegPath was provided'
        return
    }
    Assert-True (Test-Path -LiteralPath $FfmpegPath -PathType Leaf) 'provided ffmpeg exists'
    $argv = @('-hide_banner','-nostats','-loglevel','error','-f','lavfi','-i','anullsrc=r=48000:cl=mono','-t','0.25','-ac','1','-ar','48000','-c:a','pcm_f32le','-f','f32le','pipe:1')
    $result = Invoke-OwnedProcess -FilePath $FfmpegPath -Arguments $argv -TimeoutMilliseconds 5000
    Assert-Equal 0 $result.ExitCode 'lavfi exit'
    Assert-Equal 48000 $result.TotalStdoutBytes '0.25 second mono float32 byte count'
    Assert-Equal 0 $result.RetainedStdout.Length 'lavfi PCM discarded'
}

Invoke-Test 'collector version and help are noninteractive' {
    $hostPath = Get-TestHostPath
    $version = Invoke-OwnedProcess -FilePath $hostPath -Arguments @('-NoLogo','-NoProfile','-NonInteractive','-File',$collector,'-Version') -TimeoutMilliseconds 5000 -RetainStdout
    Assert-Equal 0 $version.ExitCode 'version exit'
    Assert-Equal '1.0.0' ((Convert-StrictUtf8ForTest $version.RetainedStdout).Trim()) 'tool version'
    $help = Invoke-OwnedProcess -FilePath $hostPath -Arguments @('-NoLogo','-NoProfile','-NonInteractive','-File',$collector,'-Help') -TimeoutMilliseconds 5000 -RetainStdout
    Assert-Equal 0 $help.ExitCode 'help exit'
    Assert-True ((Convert-StrictUtf8ForTest $help.RetainedStdout) -like '*микрофон*') 'Russian help'
}

Invoke-Test 'unknown collector argument fails' {
    $result = Invoke-OwnedProcess -FilePath (Get-TestHostPath) -Arguments @('-NoLogo','-NoProfile','-NonInteractive','-File',$collector,'-NotARealParameter') -TimeoutMilliseconds 5000
    Assert-True ($result.ExitCode -ne 0) 'unknown argument must fail'
}

$passed = @($script:Results | Where-Object { $_.status -eq 'pass' }).Count
$failed = @($script:Results | Where-Object { $_.status -eq 'fail' }).Count
$skipped = @($script:Results | Where-Object { $_.status -eq 'skip' }).Count
$summary = [ordered]@{
    schemaVersion = 1
    toolVersion = '1.0.0'
    startedAtUtc = $script:StartedAt.ToString('o')
    finishedAtUtc = [DateTime]::UtcNow.ToString('o')
    powershell = $PSVersionTable.PSVersion.ToString()
    platform = [Environment]::OSVersion.Platform.ToString()
    ffmpegSmokeRequested = -not [string]::IsNullOrWhiteSpace($FfmpegPath)
    passed = $passed
    failed = $failed
    skipped = $skipped
    tests = $script:Results
}

if (-not [string]::IsNullOrWhiteSpace($ReportPath)) {
    $parent = Split-Path -Parent $ReportPath
    if (-not [string]::IsNullOrWhiteSpace($parent)) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
    [IO.File]::WriteAllText($ReportPath, ($summary | ConvertTo-Json -Depth 8), (New-Object Text.UTF8Encoding($false)))
}

Remove-Item -LiteralPath $script:TestRoot -Recurse -Force -ErrorAction SilentlyContinue
Write-Host ("RESULT passed={0} failed={1} skipped={2}" -f $passed, $failed, $skipped)
if ($failed -gt 0) { exit 1 }
exit 0
