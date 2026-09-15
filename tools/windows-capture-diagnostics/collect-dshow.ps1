[CmdletBinding()]
param(
    [string]$SelectedName,
    [switch]$Help,
    [switch]$Version
)

$ErrorActionPreference = 'Stop'
$script:CaptureDiagnosticsVersion = '1.0.0'
$script:ExpectedFfmpegSha256 = '5af82a0d4fe2b9eae211b967332ea97edfc51c6b328ca35b827e73eac560dc0d'
$script:EnumerationByteLimit = 128 * 1024

function Initialize-CaptureProbe {
    if ($null -ne ('CaptureProbe' -as [type])) { return }
    $source = Join-Path $PSScriptRoot 'CaptureProbe.cs'
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
        throw "Не найден обязательный файл CaptureProbe.cs: $source"
    }
    Add-Type -Path $source
}

function Get-DShowEnumerationArguments {
    return [string[]]@('-hide_banner','-nostats','-nostdin','-list_devices','true','-f','dshow','-i','dummy')
}

function Get-DShowOptionsArguments {
    param([Parameter(Mandatory = $true)][string]$Token)
    return [string[]]@('-hide_banner','-nostats','-nostdin','-loglevel','debug','-list_options','true','-f','dshow','-i',('audio=' + $Token))
}

function Get-DShowCaptureArguments {
    param([Parameter(Mandatory = $true)][string]$Token)
    return [string[]]@(
        '-hide_banner','-nostats','-loglevel','error','-f','dshow','-i',('audio=' + $Token),
        '-map','0:a:0','-ac','1','-ar','48000','-c:a','pcm_f32le','-f','f32le','pipe:1'
    )
}

function Invoke-OwnedProcess {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string[]]$Arguments,
        [Parameter(Mandatory = $true)][ValidateRange(1, 60000)][int]$TimeoutMilliseconds,
        [switch]$RetainStdout,
        [ValidateRange(1, 10000)][int]$KillGraceMilliseconds = 2000
    )
    Initialize-CaptureProbe
    return [CaptureProbe]::Run($FilePath, $Arguments, $TimeoutMilliseconds, $RetainStdout.IsPresent, $KillGraceMilliseconds)
}

function Invoke-OwnedCaptureProcess {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string[]]$Arguments,
        [ValidateRange(1, 30000)][int]$FirstDataTimeoutMilliseconds = 10000,
        [ValidateRange(1, 30000)][int]$CaptureAfterFirstDataMilliseconds = 3000,
        [ValidateRange(1, 10000)][int]$QuitGraceMilliseconds = 2000,
        [ValidateRange(1, 10000)][int]$KillGraceMilliseconds = 2000
    )
    Initialize-CaptureProbe
    return [CaptureProbe]::RunCapture(
        $FilePath,
        $Arguments,
        $FirstDataTimeoutMilliseconds,
        $CaptureAfterFirstDataMilliseconds,
        $QuitGraceMilliseconds,
        $KillGraceMilliseconds)
}

function Test-ValidDShowField {
    param([string]$Value)
    if ([string]::IsNullOrWhiteSpace($Value)) { return $false }
    foreach ($character in $Value.ToCharArray()) {
        if ([char]::IsControl($character)) { return $false }
    }
    return $true
}

function ConvertFrom-DShowEnumerationBytes {
    [CmdletBinding()]
    param([Parameter(Mandatory = $true)][AllowEmptyCollection()][byte[]]$Bytes)

    if ($Bytes.Length -gt $script:EnumerationByteLimit) {
        throw "DirectShow device listing exceeded $script:EnumerationByteLimit bytes"
    }

    try {
        $text = (New-Object Text.UTF8Encoding($false, $true)).GetString($Bytes)
    } catch {
        throw 'DirectShow device listing was not valid UTF-8'
    }

    $pending = $null
    $devices = New-Object Collections.ArrayList
    foreach ($line in ($text -split "`r?`n")) {
        if ([string]::IsNullOrEmpty($line)) {
            if ($null -ne $pending) { throw 'DirectShow device listing is malformed' }
            continue
        }

        $lineMatch = [regex]::Match($line, '^\[dshow @ ([^\]]+)\](.*)$')
        if (-not $lineMatch.Success) {
            if ($null -ne $pending) { throw 'DirectShow device listing is malformed' }
            continue
        }

        $context = $lineMatch.Groups[1].Value
        $body = $lineMatch.Groups[2].Value.TrimStart()
        if ($body.StartsWith('Alternative name "', [StringComparison]::Ordinal)) {
            if ($null -eq $pending) { throw 'DirectShow device listing is malformed' }
            if (-not $body.EndsWith('"', [StringComparison]::Ordinal)) { throw 'DirectShow device listing is malformed' }
            $moniker = $body.Substring(18, $body.Length - 19)
            if ((-not (Test-ValidDShowField $moniker)) -or $moniker.Contains(':')) {
                throw 'DirectShow device listing is malformed'
            }
            if (-not [string]::Equals($pending.Context, $context, [StringComparison]::Ordinal)) {
                throw 'DirectShow device listing is malformed'
            }
            if ($pending.Audio) {
                [void]$devices.Add([pscustomobject]@{
                    FriendlyName = $pending.FriendlyName
                    Moniker = $moniker
                    Audio = $true
                })
            }
            $pending = $null
            continue
        }

        if ($body.StartsWith('"', [StringComparison]::Ordinal)) {
            if ($null -ne $pending) { throw 'DirectShow device listing is malformed' }
            $record = [regex]::Match($body, '^"(.*)" \(([^()]*)\)$')
            if (-not $record.Success) { throw 'DirectShow device listing is malformed' }
            $friendly = $record.Groups[1].Value
            $types = $record.Groups[2].Value
            if ((-not (Test-ValidDShowField $friendly)) -or [string]::IsNullOrEmpty($types)) {
                throw 'DirectShow device listing is malformed'
            }
            $audio = $false
            $mediaTypes = @($types -split ',' | ForEach-Object { $_.Trim() })
            foreach ($media in $mediaTypes) {
                if ($media -eq 'audio') { $audio = $true; continue }
                if ($media -eq 'video' -or $media -eq 'unknown') { continue }
                if ($media -eq 'none' -and $types -eq 'none') { continue }
                throw 'DirectShow device listing is malformed'
            }
            $pending = [pscustomobject]@{ Context = $context; FriendlyName = $friendly; Audio = $audio }
            continue
        }

        if ($null -ne $pending) { throw 'DirectShow device listing is malformed' }
    }

    if ($null -ne $pending) { throw 'DirectShow device listing is malformed' }
    return @($devices)
}

function Select-DShowDevice {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][object[]]$Devices,
        [Parameter(Mandatory = $true)][string]$ExactName
    )
    if (-not (Test-ValidDShowField $ExactName)) { throw 'Selected microphone name is malformed' }
    $matches = @($Devices | Where-Object {
        $_.Audio -and [string]::Equals([string]$_.FriendlyName, $ExactName, [StringComparison]::Ordinal)
    })
    if ($matches.Count -eq 0) { throw 'Selected microphone was not found in DirectShow' }
    if ($matches.Count -gt 1) { throw "Selected microphone is ambiguous in DirectShow ($($matches.Count) matches)" }
    return $matches[0]
}

function Normalize-InstallLocation {
    param([Parameter(Mandatory = $true)][string]$Value)
    $trimmed = $Value.Trim()
    if ([string]::IsNullOrWhiteSpace($trimmed)) { throw 'InstallLocation is empty' }
    $startsQuoted = $trimmed.StartsWith('"', [StringComparison]::Ordinal)
    $endsQuoted = $trimmed.EndsWith('"', [StringComparison]::Ordinal)
    if ($startsQuoted -ne $endsQuoted) { throw 'InstallLocation is malformed' }
    if ($startsQuoted) {
        if ($trimmed.Length -lt 3) { throw 'InstallLocation is malformed' }
        $trimmed = $trimmed.Substring(1, $trimmed.Length - 2).Trim()
    }
    if ([string]::IsNullOrWhiteSpace($trimmed)) { throw 'InstallLocation is empty' }
    return $trimmed
}

function Resolve-ConversationalyInstallLocation {
    $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Conversationaly GigaSTT Dev'
    if (Test-Path -LiteralPath $key) {
        $entry = Get-ItemProperty -LiteralPath $key
        if ($null -eq $entry.InstallLocation) { throw 'В регистрации приложения отсутствует InstallLocation.' }
        return Normalize-InstallLocation -Value ([string]$entry.InstallLocation)
    }
    if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) { throw 'Не удалось определить LOCALAPPDATA.' }
    return Join-Path $env:LOCALAPPDATA 'Conversationaly GigaSTT Dev'
}

function Assert-ConversationalyNotRunning {
    param([Parameter(Mandatory = $true)][string]$InstallLocation)
    $processes = @(Get-Process -Name 'conversationaly' -ErrorAction SilentlyContinue)
    if ($processes.Count -eq 0) { return }

    $expected = Join-Path $InstallLocation 'conversationaly.exe'
    foreach ($process in $processes) {
        $path = $null
        try { $path = $process.Path } catch { $path = $null }
        if ([string]::IsNullOrWhiteSpace($path) -or
            [string]::Equals($path, $expected, [StringComparison]::OrdinalIgnoreCase)) {
            throw 'Conversationaly запущен. Закройте его через значок в трее -> Quit и запустите диагностику снова.'
        }
    }
}

function Convert-ProbeResultForReport {
    param(
        [Parameter(Mandatory = $true)][string]$Kind,
        [Parameter(Mandatory = $true)]$Result,
        [switch]$IncludeStdoutText
    )
    $stderrText = $null
    $stderrUtf8 = $true
    try { $stderrText = (New-Object Text.UTF8Encoding($false, $true)).GetString([byte[]]$Result.RetainedStderr) }
    catch { $stderrUtf8 = $false }
    $stdoutText = $null
    if ($IncludeStdoutText) {
        try { $stdoutText = (New-Object Text.UTF8Encoding($false, $true)).GetString([byte[]]$Result.RetainedStdout) }
        catch { $stdoutText = $null }
    }

    return [ordered]@{
        kind = $Kind
        argv = @($Result.Arguments)
        started = [bool]$Result.Started
        startedAtUtc = $(if ($Result.StartedAtUtc) { ([DateTime]$Result.StartedAtUtc).ToString('o') } else { $null })
        finishedAtUtc = $(if ($Result.FinishedAtUtc) { ([DateTime]$Result.FinishedAtUtc).ToString('o') } else { $null })
        durationMilliseconds = $Result.DurationMilliseconds
        firstStdoutByteMilliseconds = $Result.FirstStdoutByteMilliseconds
        exitCode = $Result.ExitCode
        outcome = $Result.Outcome
        stopRequested = [bool]$Result.StopRequested
        forcedStop = [bool]$Result.ForcedStop
        timedOut = [bool]$Result.TimedOut
        totalStdoutBytes = $Result.TotalStdoutBytes
        totalStderrBytes = $Result.TotalStderrBytes
        stdoutRetainedBytes = @($Result.RetainedStdout).Count
        stderrRetainedBytes = @($Result.RetainedStderr).Count
        stdoutTruncated = [bool]$Result.StdoutTruncated
        stderrTruncated = [bool]$Result.StderrTruncated
        retainedStdoutText = $stdoutText
        retainedStderrUtf8 = $stderrUtf8
        retainedStderrText = $stderrText
        retainedStderrBase64 = [Convert]::ToBase64String([byte[]]$Result.RetainedStderr)
        cleanupComplete = [bool]$Result.CleanupComplete
        cleanupError = $Result.CleanupError
        stopWriteError = $Result.StopWriteError
        jobAssigned = [bool]$Result.JobAssigned
        launchError = $Result.LaunchError
    }
}

function Invoke-DiagnosticSequence {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$FfmpegPath,
        [string]$SelectedName,
        [scriptblock]$ProbeInvoker
    )

    if ($null -eq $ProbeInvoker) {
        $ProbeInvoker = {
            param([string]$Kind, [string]$FilePath, [string[]]$Arguments, [bool]$RetainStdout)
            if ($Kind -like 'capture-*') {
                return Invoke-OwnedCaptureProcess -FilePath $FilePath -Arguments $Arguments
            }
            return Invoke-OwnedProcess -FilePath $FilePath -Arguments $Arguments -TimeoutMilliseconds 5000 -RetainStdout:$RetainStdout
        }
    }

    $processes = New-Object Collections.ArrayList
    $versionResult = & $ProbeInvoker 'version' $FfmpegPath ([string[]]@('-version')) $true
    [void]$processes.Add([pscustomobject]@{ Kind = 'version'; Result = $versionResult; IncludeStdoutText = $true })
    if (-not [string]::IsNullOrWhiteSpace([string]$versionResult.LaunchError)) {
        return [pscustomobject]@{ Selection = $null; Processes = @($processes); Error = 'FFmpeg version probe failed to launch' }
    }

    $enumArgs = Get-DShowEnumerationArguments
    $enumResult = & $ProbeInvoker 'enumeration' $FfmpegPath $enumArgs $false
    [void]$processes.Add([pscustomobject]@{ Kind = 'enumeration'; Result = $enumResult; IncludeStdoutText = $false })
    if (-not [string]::IsNullOrWhiteSpace([string]$enumResult.LaunchError)) {
        return [pscustomobject]@{ Selection = $null; Processes = @($processes); Error = 'DirectShow enumeration failed to launch' }
    }
    if ([bool]$enumResult.StderrTruncated -or [long]$enumResult.TotalStderrBytes -gt $script:EnumerationByteLimit) {
        return [pscustomobject]@{
            Selection = $null
            Processes = @($processes)
            Error = "DirectShow device listing exceeded $script:EnumerationByteLimit bytes"
        }
    }

    try {
        $devices = @(ConvertFrom-DShowEnumerationBytes -Bytes ([byte[]]$enumResult.RetainedStderr))
    } catch {
        return [pscustomobject]@{ Selection = $null; Processes = @($processes); Error = $_.Exception.Message }
    }
    if ($devices.Count -eq 0) {
        return [pscustomobject]@{ Selection = $null; Processes = @($processes); Error = 'DirectShow did not report any audio-capable device' }
    }

    $exactName = $SelectedName
    if ([string]::IsNullOrWhiteSpace($exactName)) {
        Write-Host ''
        Write-Host 'Найдены микрофоны:'
        for ($i = 0; $i -lt $devices.Count; $i++) {
            Write-Host ("  {0}. {1}" -f ($i + 1), $devices[$i].FriendlyName)
        }
        $answer = Read-Host 'Введите номер нужного микрофона (Enter отменяет)'
        if ([string]::IsNullOrWhiteSpace($answer)) {
            return [pscustomobject]@{ Selection = $null; Processes = @($processes); Error = 'Выбор микрофона отменён' }
        }
        $number = 0
        if ((-not [int]::TryParse($answer, [ref]$number)) -or $number -lt 1 -or $number -gt $devices.Count) {
            return [pscustomobject]@{ Selection = $null; Processes = @($processes); Error = 'Указан неверный номер микрофона' }
        }
        $exactName = [string]$devices[$number - 1].FriendlyName
    }

    try { $selection = Select-DShowDevice -Devices $devices -ExactName $exactName }
    catch { return [pscustomobject]@{ Selection = $null; Processes = @($processes); Error = $_.Exception.Message } }

    $identities = @(
        [pscustomobject]@{ Suffix = 'moniker'; Token = [string]$selection.Moniker },
        [pscustomobject]@{ Suffix = 'friendly'; Token = [string]$selection.FriendlyName }
    )
    foreach ($identity in $identities) {
        $kind = 'options-' + $identity.Suffix
        $args = Get-DShowOptionsArguments -Token $identity.Token
        $result = & $ProbeInvoker $kind $FfmpegPath $args $false
        [void]$processes.Add([pscustomobject]@{ Kind = $kind; Result = $result; IncludeStdoutText = $false })
    }
    foreach ($identity in $identities) {
        $kind = 'capture-' + $identity.Suffix
        $args = Get-DShowCaptureArguments -Token $identity.Token
        $result = & $ProbeInvoker $kind $FfmpegPath $args $false
        [void]$processes.Add([pscustomobject]@{ Kind = $kind; Result = $result; IncludeStdoutText = $false })
    }

    return [pscustomobject]@{
        Selection = [pscustomobject]@{ FriendlyName = $selection.FriendlyName; Moniker = $selection.Moniker }
        Processes = @($processes)
        Error = $null
    }
}

function Get-DiagnosticOutputParent {
    $desktop = [Environment]::GetFolderPath('DesktopDirectory')
    if (-not [string]::IsNullOrWhiteSpace($desktop)) { return $desktop }
    $documents = [Environment]::GetFolderPath('MyDocuments')
    if (-not [string]::IsNullOrWhiteSpace($documents)) { return $documents }
    if (-not [string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) { return $env:LOCALAPPDATA }
    throw 'Не удалось определить доступную пользовательскую папку для отчёта.'
}

function Write-DiagnosticArchive {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$OutputParent,
        [Parameter(Mandatory = $true)]$Report
    )
    New-Item -ItemType Directory -Path $OutputParent -Force | Out-Null
    $name = 'Conversationaly-Capture-Diagnostics-{0}-{1}' -f (Get-Date -Format 'yyyyMMdd-HHmmss'), ([Guid]::NewGuid().ToString('N').Substring(0, 8))
    $directory = Join-Path $OutputParent $name
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
    $reportPath = Join-Path $directory 'report.json'
    [IO.File]::WriteAllText($reportPath, ($Report | ConvertTo-Json -Depth 12), (New-Object Text.UTF8Encoding($false)))
    $summaryPath = Join-Path $directory 'README.txt'
    $summary = @(
        'Локальный диагностический отчёт Conversationaly.',
        ('Статус: ' + [string]$Report.status),
        ('Ошибка: ' + [string]$Report.error),
        'Записей аудио, расшифровок и настроек в этой папке нет.',
        'Передайте ZIP специалисту вручную.'
    ) -join [Environment]::NewLine
    [IO.File]::WriteAllText($summaryPath, $summary, (New-Object Text.UTF8Encoding($true)))
    $zipPath = $directory + '.zip'
    Compress-Archive -Path (Join-Path $directory '*') -DestinationPath $zipPath -CompressionLevel Optimal
    return [pscustomobject]@{ DirectoryPath = $directory; ReportPath = $reportPath; ZipPath = $zipPath }
}

function Get-WindowsIdentityRecord {
    $record = [ordered]@{
        caption = $null
        version = [Environment]::OSVersion.Version.ToString()
        build = [Environment]::OSVersion.Version.Build
        timeZone = [TimeZoneInfo]::Local.Id
    }
    try {
        $os = Get-CimInstance -ClassName Win32_OperatingSystem
        $record.caption = $os.Caption
        $record.version = $os.Version
        $record.build = $os.BuildNumber
    } catch {
        $record.caption = [Environment]::OSVersion.VersionString
    }
    return $record
}

function Show-CaptureDiagnosticsHelp {
    @'
Диагностика запуска микрофона Conversationaly GigaSTT.

Обычный запуск: дважды щёлкните START_DIAGNOSTICS.cmd и выберите точное имя
микрофона. Инструмент сделает два коротких открытия DirectShow. Звуковые
данные отбрасываются в памяти: аудиофайл не создаётся и ничего не отправляется.

Автоматизация: collect-dshow.ps1 -SelectedName "точное имя"
'@ | Write-Output
}

function Invoke-CaptureDiagnosticsMain {
    if ($Version) { Write-Output $script:CaptureDiagnosticsVersion; return 0 }
    if ($Help) { Show-CaptureDiagnosticsHelp; return 0 }

    Write-Host 'Conversationaly: безопасная диагностика запуска микрофона'
    Write-Host 'Аудио отбрасывается. Отправки в сеть и изменения настроек нет.'
    $report = [ordered]@{
        schemaVersion = 1
        toolVersion = $script:CaptureDiagnosticsVersion
        status = 'partial'
        error = $null
        createdAtUtc = [DateTime]::UtcNow.ToString('o')
        windows = Get-WindowsIdentityRecord
        ffmpeg = $null
        selection = $null
        processes = @()
    }
    $exitCode = 1
    $archive = $null
    try {
        $install = Resolve-ConversationalyInstallLocation
        $ffmpeg = Join-Path $install 'ffmpeg.exe'
        if (-not (Test-Path -LiteralPath $ffmpeg -PathType Leaf)) {
            throw "Не найден установленный FFmpeg: $ffmpeg"
        }
        Assert-ConversationalyNotRunning -InstallLocation $install
        $actualHash = (Get-FileHash -LiteralPath $ffmpeg -Algorithm SHA256).Hash.ToLowerInvariant()
        $report.ffmpeg = [ordered]@{
            path = $ffmpeg
            sha256 = $actualHash
            expectedApp145Sha256 = $script:ExpectedFfmpegSha256
            matchesExpectedApp145Sha256 = [string]::Equals($actualHash, $script:ExpectedFfmpegSha256, [StringComparison]::OrdinalIgnoreCase)
        }

        $sequence = Invoke-DiagnosticSequence -FfmpegPath $ffmpeg -SelectedName $SelectedName
        $converted = New-Object Collections.ArrayList
        foreach ($entry in $sequence.Processes) {
            [void]$converted.Add((Convert-ProbeResultForReport -Kind $entry.Kind -Result $entry.Result -IncludeStdoutText:$entry.IncludeStdoutText))
        }
        $report.processes = @($converted)
        $report.selection = $sequence.Selection
        if (-not [string]::IsNullOrWhiteSpace([string]$sequence.Error)) {
            throw $sequence.Error
        }
        $report.status = 'completed'
        $exitCode = 0
    } catch {
        $report.error = $_.Exception.Message
        Write-Host ''
        Write-Host ('Диагностика завершена с ошибкой: ' + $report.error) -ForegroundColor Yellow
    } finally {
        try {
            $parent = Get-DiagnosticOutputParent
            $archive = Write-DiagnosticArchive -OutputParent $parent -Report $report
            Write-Host ''
            Write-Host ('ZIP с результатом: ' + $archive.ZipPath) -ForegroundColor Green
            Write-Host ('Исходная папка отчёта сохранена: ' + $archive.DirectoryPath)
        } catch {
            Write-Host ('Не удалось создать ZIP: ' + $_.Exception.Message) -ForegroundColor Red
            $exitCode = 1
        }
    }
    return $exitCode
}

if ($MyInvocation.InvocationName -ne '.') {
    if ($Version) {
        Write-Output $script:CaptureDiagnosticsVersion
        exit 0
    }
    if ($Help) {
        Show-CaptureDiagnosticsHelp
        exit 0
    }
    $mainExitCode = Invoke-CaptureDiagnosticsMain
    exit $mainExitCode
}
