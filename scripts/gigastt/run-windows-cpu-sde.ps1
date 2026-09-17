[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$Sde,
    [Parameter(Mandatory=$true)][string]$RuntimeDir,
    [Parameter(Mandatory=$true)][string]$OldRuntimeDir,
    [Parameter(Mandatory=$true)][string]$ControlBinary,
    [Parameter(Mandatory=$true)][string]$Audio,
    [Parameter(Mandatory=$true)][string]$ModelDir,
    [Parameter(Mandatory=$true)][string]$EvidenceDir,
    [Parameter(Mandatory=$true)][string]$BuildRunId,
    [Parameter(Mandatory=$true)][string]$BuildCommit
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'Intel SDE gate requires native Windows' }
. (Join-Path $PSScriptRoot 'windows-cpu-sde.ps1')
$manifest = Get-Content (Join-Path $PSScriptRoot 'windows-resources.json') -Raw | ConvertFrom-Json
$profile = Get-Content (Join-Path $PSScriptRoot 'windows-cpu-compat.json') -Raw | ConvertFrom-Json
$Sde = (Resolve-Path -LiteralPath $Sde).Path
$RuntimeDir = (Resolve-Path -LiteralPath $RuntimeDir).Path
$OldRuntimeDir = (Resolve-Path -LiteralPath $OldRuntimeDir).Path
$Audio = (Resolve-Path -LiteralPath $Audio).Path
$ControlBinary = (Resolve-Path -LiteralPath $ControlBinary).Path
New-Item -ItemType Directory -Path $EvidenceDir -Force | Out-Null
$EvidenceDir = (Resolve-Path -LiteralPath $EvidenceDir).Path
function Assert-Hash([string]$Path, [string]$Expected) {
    if ((Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() -cne $Expected) { throw "Hash mismatch: $([IO.Path]::GetFileName($Path))" }
}
$inventory = Get-Content (Join-Path $RuntimeDir 'runtime-inventory.json') -Raw | ConvertFrom-Json
if ($inventory.runtimeProfile -ne 'cpu-compat-v1' -or $inventory.sourceCommit -ne $manifest.gigastt.sourceCommit) { throw 'Unexpected candidate runtime profile/source' }
foreach ($file in $inventory.packagedFiles) {
    if ($file.name -notmatch '^[A-Za-z0-9_.-]+$') { throw 'Unsafe inventory member' }
    Assert-Hash (Join-Path $RuntimeDir $file.name) $file.sha256
}
foreach ($file in $profile.ort.files) { Assert-Hash (Join-Path $RuntimeDir $file.destination) $file.sha256 }
$binary = Join-Path $RuntimeDir 'gigastt.exe'
$oldBinary = Join-Path $OldRuntimeDir 'gigastt.exe'
Assert-Hash $oldBinary '2969b894d483bda6abbf97ecc02edd435be921eadfd305f3938fedaf7f20afbd'
Assert-Hash $Audio $manifest.fixture.sha256
# Copy only pinned raw models: never reuse modern-host optimized cache files.
$freshModels = Join-Path (Split-Path -Parent $EvidenceDir) 'models-ivb'
if (Test-Path $freshModels) { throw 'Refusing to reuse a previous IVB model directory' }
foreach ($file in @($manifest.model.mainFiles) + @($manifest.model.sideFiles)) {
    $source = Join-Path $ModelDir $file.relativePath
    Assert-Hash $source $file.sha256
    $destination = Join-Path $freshModels $file.relativePath
    New-Item -ItemType Directory -Path (Split-Path -Parent $destination) -Force | Out-Null
    Copy-Item -LiteralPath $source -Destination $destination
}
$runs = [Collections.Generic.List[object]]::new()
$summary = [ordered]@{
    schemaVersion=1; status='running'; chip='IVB'; hardwareTargetAccepted=$false
    buildRunId=$BuildRunId; buildCommit=$BuildCommit
    validationCommit=$env:GITHUB_SHA
    binarySha256=(Get-FileHash $binary -Algorithm SHA256).Hash.ToLowerInvariant()
    ortVersion=$profile.ort.version; ortDllSha256=$profile.ort.files[0].sha256
    sdeVersion='10.13.1'; error=$null
}
function Run-Check([string]$Name, [string]$Exe, [string[]]$AppArguments, [bool]$Negative, [int]$Limit = 1200) {
    $directory = Split-Path -Parent $Exe
    # Include all shipped native DLLs, not only the Rust executable.
    $images = @([IO.Path]::GetFileName($Exe)) + @(Get-ChildItem -LiteralPath $directory -File -Filter '*.dll' | ForEach-Object { $_.Name })
    $args = Get-IvbArguments -Binary $Exe -Arguments $AppArguments -Images $images
    $result = Invoke-BoundedSde -Sde $Sde -Arguments $args -WorkingDirectory $directory -EvidencePrefix (Join-Path $EvidenceDir $Name) -TimeoutSeconds $Limit
    $runs.Add([ordered]@{ name=$Name; exitCode=$result.exitCode; durationMilliseconds=$result.durationMilliseconds; images=$images; expectedIllegalInstruction=$Negative })
    Assert-IvbOutcome -ExitCode $result.exitCode -Text $result.text -ExpectedIllegalInstruction:$Negative
}
function Transcribe-Arguments([string]$OutputPath) {
    return @('--offline','transcribe',$Audio,'--model-dir',$freshModels,'--model-variant','rnnt',
        '--vad','--vad-model-dir',(Join-Path $freshModels 'vad'),
        '--punctuation','on','--punct-model-dir',(Join-Path $freshModels 'punct'),
        '--itn','on','--encoder-intra-threads','1','--format','json','--output',$OutputPath)
}
try {
    Run-Check 'synthetic-negative' $ControlBinary @('control') $true 120
    Run-Check 'old-runtime-negative' $oldBinary (Transcribe-Arguments (Join-Path $EvidenceDir 'old-result.json')) $true 1200
    Run-Check 'candidate-help' $binary @('--help') $false 120
    Run-Check 'candidate-version' $binary @('--version') $false 120
    $resultPath = Join-Path $EvidenceDir 'candidate-result.json'
    Run-Check 'candidate-transcribe' $binary (Transcribe-Arguments $resultPath) $false 1800
    $result = Get-Content -LiteralPath $resultPath -Raw | ConvertFrom-Json
    Assert-IvbTranscript -Result $result
    $summary['wordCount'] = @($result.words).Count
    $summary['durationSeconds'] = $result.duration
    $summary.status = 'passed'
} catch {
    $summary.status = 'failed'; $summary.error = $_.Exception.Message
    throw
} finally {
    $summary['runs'] = $runs.ToArray()
    $summary | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $EvidenceDir 'cpu-evidence.json') -Encoding utf8
    # Only evidence is uploaded; models and public transcript are not deliverables.
    Remove-Item -LiteralPath $freshModels -Recurse -Force -ErrorAction SilentlyContinue
    foreach ($name in @('old-result.json','candidate-result.json')) { Remove-Item (Join-Path $EvidenceDir $name) -Force -ErrorAction SilentlyContinue }
}
Write-Host 'IVB INSTRUCTION GATE PASS: negative controls rejected; candidate real inference passed. Physical target acceptance remains pending.'
