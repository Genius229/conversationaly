# GigaSTT VAD comparison

The application setting **GigaSTT VAD** controls speech detection for final
GigaSTT transcription only. It does not control microphone recording, DirectShow,
live draft transcription, or Conversationaly's separate VAD.

## Intended behavior

- Default: **on**, including existing settings files that predate this setting.
- The choice is saved and used by the next final transcription, import, retry
  or explicit re-transcription. No application restart is required.
- A running job keeps the choice it started with.
- Switching the setting alone does not overwrite existing transcripts. Use
  the existing explicit re-transcription action to compare a saved recording.
- Turning it off sends the full prepared audio to the same GigaSTT model.
  Punctuation, casing/ITN and timestamps remain enabled.
- Processing silence may take more ASR work and may produce false words on
  noise. Compare the same file in both modes; do not compare different spoken
  passages as if that were a controlled accuracy test.

The runtime remains GigaSTT 2.21.0. This switch does **not** patch its Silero
wrapper, change model weights, or substitute Conversationaly's VAD. VAD models
remain loaded in the managed sidecar so the next request can select either mode.

## Before the toggle: standalone test on installed 1.4.4

Version 1.4.4 cannot disable VAD through `gigastt-settings.json` or an environment
variable: it explicitly submits `vad=true`, and the sidecar strips inherited
`GIGASTT_*` environment overrides. Adding an unknown JSON field is not a fix.

For a separate diagnostic without updating that application, its bundled CLI
can process a saved recording with VAD disabled. This creates a new TXT only;
it does not replace the transcript in the application database. It uses already
installed models offline. Close Conversationaly first to avoid loading another
copy of the recognition model alongside the app.

Run in **PowerShell**, not `cmd.exe`. This example is for the
`Conversationaly GigaSTT Dev` current-user installation and preserves existing
process-local environment variables after the command finishes:

```powershell
$ErrorActionPreference = 'Stop'
if (Get-Process -Name conversationaly,gigastt -ErrorAction SilentlyContinue) {
  throw 'Close Conversationaly/GigaSTT first.'
}
$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Conversationaly GigaSTT Dev'
$install = ([string](Get-ItemProperty -LiteralPath $key).InstallLocation).Trim('"')
$exe = Join-Path $install 'gigastt\gigastt.exe'
$models = Join-Path $env:APPDATA 'com.conversationaly.gigastt-dev\models\gigastt\2.18.0'
$inputPath = (Read-Host 'Full path to saved audio.mp4 or WAV').Trim().Trim('"')
$audio = (Resolve-Path -LiteralPath $inputPath).Path
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw "Missing executable: $exe" }
if (-not (Test-Path -LiteralPath $models -PathType Container)) { throw "Missing models: $models" }
$outDir = Join-Path ([Environment]::GetFolderPath('MyDocuments')) 'Conversationaly-no-VAD'
New-Item -ItemType Directory -Path $outDir -Force | Out-Null
$out = Join-Path $outDir ('gigastt-no-vad-' + [guid]::NewGuid().ToString() + '.txt')
$savedEnv = @{}
Get-ChildItem Env: | Where-Object Name -Like 'GIGASTT_*' | ForEach-Object {
  $savedEnv[$_.Name] = $_.Value
  Remove-Item -LiteralPath ('Env:' + $_.Name)
}
try {
  & $exe --log-level error --offline transcribe $audio `
    --model-dir $models --model-variant rnnt `
    --punctuation on --punct-model-dir (Join-Path $models 'punct') `
    --itn on --file-window-concurrency 1 --format txt --output $out
  if ($LASTEXITCODE -ne 0) { throw "GigaSTT exit code: $LASTEXITCODE" }
  Write-Host "Saved: $out"
  Get-Content -LiteralPath $out -Raw -Encoding UTF8
} finally {
  foreach ($name in $savedEnv.Keys) {
    Set-Item -LiteralPath ('Env:' + $name) -Value $savedEnv[$name]
  }
}
```

Omitting `--vad` is intentional: standalone `transcribe` defaults to VAD off.
The `2.18.0` directory is the existing model layout, not the runtime version.
The CLI writes UTF-8 directly through `--output`, avoiding PowerShell redirection
encoding differences. The command was checked against v2.21.0 source; actual
hardware/runtime transcription quality must still be tested on the target PC.
