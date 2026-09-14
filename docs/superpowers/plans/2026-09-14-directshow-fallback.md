# Windows DirectShow fallback implementation plan

> **For agentic workers:** Use superpowers:subagent-driven-development and independent scoped reviews. Tasks share the interface below; only edit assigned files.

**Goal:** Add FFmpeg/DirectShow microphone capture only after an eligible WASAPI startup failure, then deliver a verified public Windows diagnostic preview.

**Architecture:** Preserve the existing CPAL negotiation fast path. A separate owned capture module opens the exact uniquely mapped DirectShow audio endpoint, converts its default input format to mono 48 kHz float PCM and feeds the existing AudioCapture pipeline. The fallback owns its process, cancellation, startup readiness, pipe draining and bounded shutdown; it is not a new transcription engine.

**Tech stack:** Existing Rust/Tokio, CPAL 0.18.1, bundled FFmpeg, Tauri/NSIS, headless Rust contracts and native Windows CI.

**Spec / user approval:** In-thread approved on 2026-09-14: user explicitly chose DirectShow as fallback for the problem Windows PC instead of expanding the BLE continuity investigation, then requested `продолжай` and subagents/Superpowers. The earlier direct command on the problem USB endpoint successfully captured 5 seconds. This plan makes that approved fallback precise; no additional UI or model behavior is included.

## Global constraints

- Windows microphone only, after exhausted CPAL format negotiation ends with `E_INVALIDARG` or supported format/config rejection. No fallback for denied access, busy/missing devices, lookup errors, system loopback, runtime Bluetooth starvation or an already-running stream.
- Keep working CPAL devices and system capture unchanged. GigaSTT 2.21.0/model assets/settings/database/diarization unchanged; no downloads or new Python/Docker/WSL production dependencies.
- Resolve the actual CPAL microphone's full friendly name; require an exact unique DirectShow audio match, reject missing/ambiguous names, prefer its associated unique alternative moniker. Never use default input, substring matches or a similarly named camera. No shell command interpolation.
- Use only an existing FFmpeg, preferably bundled. DirectShow native input defaults; output `pcm_f32le`, mono, 48000 Hz over stdout. Continually drain stderr with a byte bound and emit sanitized categories/counts, not raw monikers/audio/text.
- Startup is successful only after real aligned finite PCM arrives. Finite enumeration/startup deadlines, cancellation-safe ownership and Drop cleanup. Windows Job Object contains the child before readiness, and explicit app-exit cleanup drains it. Sudden parent death in the short spawn-to-job-assignment interval is not claimed atomically covered; assignment failure itself must kill/reap with a deadline.
- Stop requests FFmpeg `q`, drains final PCM, then reaps; finite grace followed by killing only the owned process. No broad process-name killing. Avoid self-join and cancellation leaks. No temporary audio files.
- Preserve fallback tail by selectively draining it BEFORE RecordingState clears the pipeline sender. Keep CPAL stop order unchanged; do not fix unrelated recorder time compression/gaps or saver behavior in this task. Active DirectShow reconnect is rejected with stop-and-start-again guidance; CPAL reconnection explicitly disables DirectShow so no in-recording backend switch or long FFmpeg await is introduced under the legacy manager mutex.
- Pause continues draining/discarding via existing RecordingState gating, never accumulating paused audio. Do not claim timestamp-perfect pause boundaries or long-duration hardware acceptance without testing.
- User audio/log archive stays outside Git. Existing untracked BLE report stays untouched in the original checkout. Work in `.worktrees/dshow-fallback`.

## Planned interfaces

- `audio/directshow/capture.rs`: async `DirectShowCapture::start(path, exact_name, on_pcm, on_error) -> Result<Self>`; `request_stop(&self)` plus async `finish(self)`/`stop(self)`; emergency Drop cancellation. The process owner runs on its own thread/runtime; finish awaits joining without blocking the caller's Tokio executor. `OUTPUT_RATE=48000`, `OUTPUT_CHANNELS=1`.
- First-packet readiness uses a cancellation-safe owned handle existing before awaiting. A stop/drain helper extracts the DirectShow microphone, requests stop, synchronously stops remaining CPAL streams, then awaits the owner drain before state closure; CPAL-only stop order remains untouched. App Exit explicitly stops external capture before teardown because static globals are not dropped reliably.
- `audio/directshow/device.rs`: pure bounded enumeration parser/unique selector and argument construction; testable with hostile/duplicate/Cyrillic fixtures. Exact parser contract to be finalized from FFmpeg source review before implementation.
- `audio/directshow/owned_child.rs`: Windows Job Object RAII (kill-on-close) and platform-independent worker helper if splitting is useful; no caller-visible raw process handle.
- Tests use an explicitly injected local fake FFmpeg command, not environment switches in production. Fake binary lives in test support, excluded from release. Runtime tests also exercise real FFmpeg generated audio (not microphone hardware) on Windows.

## Tasks

### Task 1: Device identity and command contract
- [x] Review FFmpeg enumeration source and confirm exact friendly/audio/alternative-name grammar.
- [x] Write RED tests: audio vs video, duplicates, missing/empty/malformed names, Cyrillic, quoted names, argument injection, bounded enumeration input.
- [x] Implement pure parser/selector/args; GREEN 10/10 + independent source review; FFmpeg `none`/`unknown` and context-pairing follow-ups covered.

### Task 2: Owned DirectShow process and lifecycle
- [x] Finalize owner-thread/runtime API with lifecycle review.
- [x] RED tests for first PCM gating, startup EOF/hang, cancellation before ready, fragmented PCM, nonfinite/truncated output, stderr flood, unexpected exit, graceful final tail, ignored quit/owned kill, Drop, repeated start/stop, no leaked child.
- [x] Implement bounded child owner, parser/read loop and diagnostics; enforce Windows Job Object containment if available, failing cleanly if ownership cannot be established.
- [x] Test with fake child on Linux/Windows and real generated PCM through bundled FFmpeg in Windows CI; independent review. Windows Job parent-exit test passed.

### Task 3: Application wiring and release
- [x] Add Windows DirectShow variant and eligible-failure branch. Map the actual resolved name, output rate/channels and errors into existing AudioCapture.
- [x] Add selective pre-drain calls to normal stop/cleanup/Exit paths. Reject active DirectShow reconnect before awaits; keep CPAL reconnect's sync stop and explicitly forbid fallback on reconnect. Test routing and stop-order contracts; retain default CPAL behavior.
- [x] Add headless harness/CI hooks; bump app version consistently (1.4.4), unchanged ASR/models.
- [x] Full locked contracts/Clippy/frontend/type/build tests; independent final review.
- [x] Merge scoped commits back to the release branch, build Windows EXE with existing native/codec/installer gates, publish public preview and SHA256, verify anonymous download, document real-hardware acceptance still pending.

## Pre-flight review

| Shared boundary | Producer | Consumer | Required invariant |
| --- | --- | --- | --- |
| Device selection | Task 1 parser | Task 2 process | Exact unique audio moniker, no default/substring fallback |
| PCM callback | Task 2 owner | Task 3 pipeline | Finite aligned mono F32 at 48 kHz, bounded pending data |
| Stop | Task 3 manager | Task 2 owner | Drain before sender closure, bounded owned process cleanup |
| Build dependencies | Task 2 Windows ownership | Task 3 CI | Desktop and headless Windows crate pins match |

Baseline: isolated branch starts at `d8c33ad`. Authorized headless suite: **139 passed**. An initial sandboxed baseline could not bind loopback sockets (EPERM), so it is not a source regression. No application code changed yet.
