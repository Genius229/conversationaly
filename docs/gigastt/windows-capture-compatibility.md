# Windows capture compatibility and diagnostics (1.4.3 preview)

## Evidence and scope

User: Windows 11, public 1.4.2, AI Speakerphone connected over Bluetooth.
Windows Sound Recorder successfully records from that exact microphone.
Uploaded application log reports `IAudioClient::Initialize` failure with
`E_INVALIDARG / 0x80070057 / -2147024809`, before any GigaSTT transcription.
The observed Speakerphone format is mono 16 kHz F32; an earlier HP Webcam
attempt on the same host also failed at 48 kHz stereo F32. This is not proof
of a particular defective Bluetooth profile or driver parameter.

Approved bounded change: try other formats advertised by the same resolved
endpoint after a format-specific initialization error, and log enough metadata
to identify the successful configuration or all rejected attempts. GigaSTT,
models, diarization, recording preferences and installer behavior are unchanged.

## Behavior

- Default configuration remains the first attempt; success does not enumerate
  formats or create additional streams.
- Retry only for CPAL `UnsupportedConfig`, `InvalidInput` or WASAPI
  `E_INVALIDARG`. Permissions, busy/disconnected devices and unknown failures
  do not trigger format retries. Failure of `play()` is logged but not retried.
- On rejection, refresh the same endpoint's default and enumerate its supported
  formats. Loopback uses output-format enumeration, not microphone enumeration.
- Try a changed refreshed default first; otherwise prefer the original channel
  count/rate and PCM16, then other advertised
  configurations. Never synthesize a rate outside the reported supported range.
- Deduplicate attempts; at most 12 builds and 64 enumerated ranges. These are
  count bounds, not a claim that a hung Windows driver call has a time deadline.
- Each attempted format gets separate capture state with the actual rate and
  channel count; failed builds are dropped. Never change microphone endpoints
  while negotiating formats. macOS/Linux keep the original single-build path.

## Diagnostic markers

The ordinary application log contains:

- Startup application version, OS/architecture and diagnostics revision.
- `capture_resolve`: requested endpoint and device/config lookup failures.
- `capture_open`: selected endpoint name/type/interface, default CPAL configuration,
  WASAPI backend, capture/loopback mode, CPAL pin and per-open correlation ID.
- `capture_attempt`: format, rate, channels, shared/event/default-buffer mode,
  attempt number, cumulative elapsed milliseconds, stage and result.
- `capture_formats` / `capture_default_refresh`: advertised alternatives or
  the enumeration error, queried only after a retryable build failure.
- `capture_selected` / `capture_failed`: final configuration or attempts used.
- `capture_play`: stream start result; `capture_first_callback`: first packet
  reaching the active recording pipeline (frame count, never sample content).
- `capture_callback_error`: asynchronous driver error, active rate/channels,
  CPAL error kind, decimal OS error and hexadecimal HRESULT when available.

No additional audio files, sample values, speech text or network telemetry are
produced by these diagnostics. Device names are included in the local log;
persistent endpoint IDs and arbitrary structured device metadata are omitted.
Do not commit the user's log or recordings.

## Verification and hardware boundary

Headless regressions exercise the production negotiation helper: default fast
path, F32 rejection/PCM16 success, terminal errors, mid-fallback disconnect,
deduplication, bounded attempts and localized HRESULT extraction. The Windows
contract job compiles the actual CPAL adapter with desktop-matching pins.
Full Windows desktop/codec/installer gates are required before publication.

The actual Speakerphone is not connected to CI. A successful build does not
prove the device problem is resolved. User acceptance: close Windows Sound
Recorder, install the preview, select the same Speakerphone, try a short
recording, then send the new `conversationaly.log` regardless of outcome. If
initialization still fails, use the logged supported configurations and HRESULTs
to determine the next targeted WASAPI/driver investigation.

## Published artifact and automated evidence

- Public tag: `gigastt-desktop-v1.4.3-preview.1`, target
  `9a9b3b30e7e86b3fe041751271a2c790215dc38b`.
- Full Windows run **34840350298 SUCCESS**, artifact **10346356763**.
  Nine installation/upgrade scenarios, ten bundled-FFmpeg codec tests, both
  native GigaSTT formatting projections passed.
- Contracts **34840350176 SUCCESS**: Ubuntu **139**, Windows **136** tests,
  **80** Windows stress repetitions and Clippy. Includes three Windows tests
  using the actual CPAL adapter and ten cross-platform negotiation regressions.
- Local frontend **31**, TypeScript and Next production build passed; independent
  source review approved. Initial Windows harness lacked direct `anyhow`, fixed
  in a test-only follow-up without changing the application's dependency graph.
- Installer **62,316,972 bytes**; SHA256
  `3d1a277cddcfc545d1c9c0253de74a641d8c1aeb4d2e6e82f177c861c89d6ab1`.
- EXE and SHA256SUMS downloaded anonymously with `curl -q`, matching the tested
  artifact and acceptance-harness hash exactly.

[Public EXE](https://github.com/Genius229/conversationaly/releases/download/gigastt-desktop-v1.4.3-preview.1/Conversationaly-GigaSTT-1.4.3-x64-setup.exe)

Hardware status: **awaiting user's Speakerphone log from 1.4.3**.
