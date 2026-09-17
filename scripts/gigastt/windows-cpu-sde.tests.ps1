[CmdletBinding()]
param([string]$ReportPath = '')
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$helper = Join-Path $PSScriptRoot 'windows-cpu-sde.ps1'
if (Test-Path $helper) { . $helper }
$results = [Collections.Generic.List[object]]::new()
function Check([string]$Name, [scriptblock]$Action) {
    try { & $Action; $results.Add(@{ name=$Name; passed=$true }); Write-Host "PASS $Name" }
    catch { $results.Add(@{ name=$Name; passed=$false; error=$_.Exception.Message }); Write-Host "FAIL $Name : $($_.Exception.Message)" }
}
function Assert($Value, [string]$Message) { if (-not $Value) { throw $Message } }
function Reject([scriptblock]$Action, [string]$Pattern) {
    try { & $Action; throw 'unexpected acceptance' }
    catch { if ($_.Exception.Message -notmatch $Pattern) { throw } }
}
Check 'IVB arguments include dynamic ORT images and no exe-only shortcut' {
    $a = @(Get-IvbArguments -Binary 'C:\runtime\gigastt.exe' -Arguments @('--help') -Images @('gigastt.exe','onnxruntime.dll','onnxruntime_providers_shared.dll'))
    Assert ($a[0] -eq '-ivb') 'missing IVB profile'
    Assert ('-chip_check_exe_only' -notin $a) 'unsafe exe-only scope'
    Assert (($a | Where-Object { $_ -eq '-chip_check_image' }).Count -eq 3) 'must include both ORT DLLs'
    Assert (($a[-3..-1] -join '|') -eq '--|C:\runtime\gigastt.exe|--help') 'argument vector changed'
}
Check 'invalid image selector and omitted executable are rejected' {
    Reject { Get-IvbArguments -Binary 'gigastt.exe' -Arguments @('--help') -Images @('gigastt.exe','*.dll') } 'Invalid image'
    Reject { Get-IvbArguments -Binary 'gigastt.exe' -Arguments @('--help') -Images @('onnxruntime.dll') } 'Executable missing'
}
Check 'negative control requires instruction-specific failure not arbitrary crash' {
    Assert-IvbOutcome -ExitCode 1 -Text 'SDE-ERROR: Executed instruction not valid for specified chip' -ExpectedIllegalInstruction
    Reject { Assert-IvbOutcome -ExitCode 0 -Text 'SDE-ERROR: Executed instruction not valid for specified chip' -ExpectedIllegalInstruction } 'Negative control'
    Reject { Assert-IvbOutcome -ExitCode 1 -Text 'Cannot load DLL' -ExpectedIllegalInstruction } 'Negative control'
}
Check 'positive control rejects exit failure and hidden SDE errors' {
    Assert-IvbOutcome -ExitCode 0 -Text 'normal model log'
    Reject { Assert-IvbOutcome -ExitCode 1 -Text '' } 'Candidate'
    Reject { Assert-IvbOutcome -ExitCode 0 -Text 'SDE-ERROR: hidden fault' } 'Candidate'
}
Check 'CLI JSON contract requires words and duration but no API-only segments' {
    $text = [string][char]0x041F + [string][char]0x0440 + [string][char]0x0438 + [string][char]0x0432 + [string][char]0x0435 + [string][char]0x0442 + '.'
    $value = [pscustomobject]@{ text=$text; duration=4.0; words=@([pscustomobject]@{text=$text;start=0.0;end=1.0}) }
    Assert-IvbTranscript -Result $value
    $value.words = @()
    Reject { Assert-IvbTranscript -Result $value } 'transcript'
}
$failed = @($results | Where-Object { -not $_.passed }).Count
$report = @{ passed=$results.Count-$failed; failed=$failed; tests=$results.ToArray() }
if ($ReportPath) { $report | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $ReportPath -Encoding utf8 }
Write-Host "Tests: $($report.passed) passed, $failed failed"
if ($failed) { exit 1 }
exit 0
