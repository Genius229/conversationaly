# DirectShow microphone startup fallback (1.4.4 preview)

## Scope and behavior

This adds the alternate capture route demonstrated by the user's successful
five-second FFmpeg/DirectShow test on the problem USB-receiver PC. It does not
replace GigaSTT, the application mixer, models or working CPAL devices.

1. Resolve and open the microphone through the existing CPAL/WASAPI format
   negotiation first. Success does not launch FFmpeg or enumerate DirectShow.
2. Only after all eligible formats fail with `UnsupportedConfig` or native
   `E_INVALIDARG`, consider DirectShow. No system-loopback fallback, denied-
   permission bypass, retry of unavailable/busy devices, play-error fallback
   or runtime backend switching is added.
3. Verify the actual resolved CPAL name exactly equals the selected name and
   is unique in the WASAPI input list. Keep the old CPAL resolver unchanged
   for working paths, but reject its ambiguous/substring/default resolution
   before using the alternate backend.
4. Enumerate DirectShow audio-capable devices through the bundled sibling
   `ffmpeg.exe`. Require one exact friendly-name match and its associated
   alternative moniker. Missing/ambiguous/incomplete listings fail closed.
   Commands use individual arguments without a shell. No PATH/cwd search or
   runtime download is used for this fallback.
5. Capture with the device's native input defaults; FFmpeg outputs mono 48 kHz
   little-endian float PCM to the existing AudioCapture pipeline. Do not force
   the previously rejected WASAPI input format onto the DirectShow device.

The app still performs its normal preprocessing/mixing and GigaSTT final
transcription. The user needs no manual console command or new model download.

## Process ownership and stop

The external capture owner has its own OS thread/current-thread Tokio runtime,
so cleanup does not depend on the caller continuing to poll a startup future.
Startup is acknowledged only after valid finite PCM is delivered. Enumeration
and first-PCM waits have separate deadlines (5 and 10 seconds by default).
The PCM decoder bounds its retained data to a 480-sample callback frame and
up to three incomplete bytes; aligned residual samples are delivered on EOF.

Normal stop requests `q`, continues draining stdout, then reaps the owned
child. A two-second grace is followed by bounded kill/reap of that child only.
Unexpected child/pipe/PCM failures report one sanitized runtime error; no raw
FFmpeg stderr, stable device monikers, speech text or samples are logged.

The manager selectively drains DirectShow before clearing RecordingState and
its pipeline sender. CPAL-only stop ordering remains unchanged. Explicit app
Exit uses the same ownership rules. DirectShow reconnection during a recording
is rejected with guidance to stop and start again; existing CPAL reconnects
explicitly disable the fallback. This avoids runtime backend switching and
adding FFmpeg waits beneath the pre-existing reconnect manager mutex. On Windows, a
kill-on-close Job Object contains each child before capture readiness. The
brief spawn-to-job-assignment interval is not claimed atomically protected
against abrupt parent death; assignment failures themselves use bounded cleanup.

This change does not fix unrelated timestamp compression during Bluetooth
packet starvation or the old recorder saver queue race. Do not claim lost
radio audio can be recovered by a startup-only fallback. File-pause alignment
and physical long-duration behavior remain hardware acceptance items.

## Verification requirements

- Pure parser tests against actual FFmpeg 8.x listing grammar: audio/video/
  mixed/none/unknown records, duplicates, Cyrillic/quotes/metacharacters,
  malformed/control/oversize/UTF-8 failures and associated-context checks.
- Fake-process lifecycle tests: PCM readiness, bounded enumeration/startup,
  cancellation/Drop, fragmented/nonfinite/truncated PCM, stderr flood,
  graceful final tail, stalled quit, unexpected exit and repeated cycles.
- Windows parent-exit-after-readiness test for Job Object containment.
- Headless source-routing guards and Windows adapter eligibility/name tests;
  strict bundled-FFmpeg resolver tests also run in the codec harness.
- Real installer FFmpeg verifies compiled DirectShow support and emits a
  generated finite PCM tone through the actual incremental decoder, including
  an incomplete final callback frame. This is NOT a microphone hardware test.
- Existing locked contracts/Clippy/frontend/type/build/native/codec/installer
  gates must pass before publishing the public manual-install preview.

## User acceptance

On the original problem PC, select the same microphone and start a short
recording. The normal WASAPI failure should be followed by
`capture_fallback backend=DirectShow`, valid-PCM readiness and capture success.
Speak, stop, play back the saved audio, then repeat start/stop. Send the new
application log on success or failure. Check microphone and system audio
separately: only the microphone has a DirectShow fallback.

## Verified public artifact

- Source: `f11ebab382d650d132d5a444316da9940fecc409`; all three task reviews
  approved after scoped corrections. Production capture/identity/wiring code
  is separate from test fixtures and does not require Python at runtime.
- Full Windows build **34894443916 SUCCESS**, installer artifact **10368807115**.
- Windows and Ubuntu contracts **34894443793 SUCCESS**: **173 tests per OS**,
  Clippy, and **80** repeated Windows lifecycle checks. Windows Job Object
  parent-exit-after-readiness test passed on Windows, not just in simulation.
- Actual bundled FFmpeg DirectShow support/PCM-tail test **PASS**. Existing
  GigaSTT native formatting checks, frontend **31** tests, **12** codec/resolver
  tests and **9** real Windows installer scenarios passed.
- Public tag: `gigastt-desktop-v1.4.4-preview.1`, targeting that exact source.
- EXE: `Conversationaly-GigaSTT-1.4.4-x64-setup.exe`, **62,428,318 bytes**.
- SHA256: `e9f434bfd41eb7924538f44ab7fb8c629ee42c81a1eedd6f056ece869e693432`.
- Anonymous `curl -q` EXE/SHA256SUMS download matches the tested installer.

[Public EXE download](https://github.com/Genius229/conversationaly/releases/download/gigastt-desktop-v1.4.4-preview.1/Conversationaly-GigaSTT-1.4.4-x64-setup.exe)

No claim is made yet that the automatic in-app fallback has been accepted on
the user's original hardware; the previous direct-command test only proved
that DirectShow can capture from that endpoint.
