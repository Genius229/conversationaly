# Conversationaly + GigaSTT Post-Recording Transcription Design

**Date:** 2026-09-09

## Goal

Fork Conversationaly into a Windows-first meeting recorder for room meetings captured through one selected external microphone, while using GigaSTT as the authoritative high-quality Russian transcription engine after recording stops. Preserve Conversationaly's meeting history, incremental audio persistence, transcript UI, summary/post-processing infrastructure, and optional existing live preview.

## Product flow

1. User selects an external microphone.
2. User starts a meeting recording.
3. Conversationaly's existing recording path writes audio incrementally to disk and retains crash/recovery behavior.
4. Live transcription is optional and non-authoritative. It may remain available as a preview, but is disabled by default in the target profile.
5. User presses Stop.
6. Conversationaly finalizes the meeting audio.
7. A post-recording transcription orchestrator prepares a GigaSTT-compatible audio file if needed.
8. GigaSTT performs full-file batch transcription with VAD, punctuation, Russian ITN, timestamps/confidence, and optional diarization.
9. The GigaSTT result atomically replaces the draft/live transcript for the meeting.
10. Existing summary/API post-processing consumes only the final GigaSTT transcript.
11. UI shows progress, cancellation, success/failure, and allows re-transcription later.

## Scope for v1

### In scope

- Windows x86-64 as the primary packaged platform.
- Single selected external microphone as the default recording source.
- Keep existing system-audio capture capability in code; do not make it required for the target workflow.
- Existing incremental recording and recovery remain unchanged unless an integration bug requires a targeted change.
- GigaSTT as a post-recording provider.
- Automatic final transcription after Stop.
- Manual re-transcription of an existing meeting through GigaSTT.
- Final transcript imported into Conversationaly's existing meeting/transcript persistence.
- Progress/cancel/error states in UI.
- Existing summary providers remain usable after final transcription.
- Preserve original finalized meeting audio.
- Temporary STT audio is removable after successful import.

### Explicitly out of scope for v1

- GigaSTT real-time streaming.
- Replacing Conversationaly's existing live STT architecture.
- Mandatory system-audio capture.
- New cloud STT provider.
- Speaker identity recognition across meetings.
- New CRM/Telegram/Bitrix integrations; only retain an extension point for later post-processing.
- GPU acceleration for GigaSTT on Windows unless a stable supported path is proven during implementation.

## Existing Conversationaly boundaries to preserve

Conversationaly already has separate recording and transcription paths. Recording should remain the source of truth for audio durability. Live transcription should remain isolated behind the existing `Transcriber` / `TranscriptSink` boundaries and must not be rewritten merely to add GigaSTT.

Relevant existing areas:

- `frontend/src-tauri/src/audio/recording_manager.rs` — recording orchestration.
- `frontend/src-tauri/src/audio/recording_saver.rs` — incremental recording/finalization.
- `frontend/src-tauri/src/audio/transcription/` — current live-transcription hexagon.
- `frontend/src-tauri/src/audio/retranscription.rs` — post-hoc transcription/replacement behavior; primary integration reference.
- `frontend/src-tauri/src/audio/diarization.rs` — existing post-hoc speaker labeling.
- `frontend/src-tauri/src/database/` — meeting/transcript persistence.
- `frontend/src-tauri/src/lib.rs` — Tauri command registration.
- `frontend/src/components/Sidebar/SidebarProvider.tsx` and meeting UI components — progress/state refresh.

## GigaSTT integration strategy

### Decision: isolated sidecar, not in-process embedding for v1

GigaSTT should run as a separate process managed by Conversationaly rather than linking `gigastt-core` directly into the main Tauri Rust binary.

Reasons:

- Keeps ONNX Runtime and GigaSTT dependency/lifecycle failures outside the recorder process.
- Allows GigaSTT to be upgraded independently from the main app.
- Avoids dependency and native-runtime coupling with Conversationaly's transcribe.cpp/llama.cpp stack.
- Matches the existing architectural precedent of the bundled `llama-helper` sidecar.
- Enables GigaSTT async jobs, progress and cancellation without inventing another IPC protocol.

### Windows packaging decision

GigaSTT's main CLI documentation currently focuses its prebuilt CLI/server releases on macOS/Linux, but GigaSTT 2.18.0 publishes a native `win_amd64` Python/UniFFI wheel, which demonstrates that the core engine and ONNX runtime path build for Windows x86-64.

For the product build, do **not** ship Python. Instead:

1. Pin a GigaSTT source/crate version (initial target: 2.18.0; refresh only deliberately).
2. Build the native `gigastt` server/CLI on a Windows MSVC runner.
3. Smoke-test `gigastt.exe` against a Russian WAV fixture.
4. Bundle the resulting `gigastt.exe` and required native runtime files as a Tauri external binary/resource.
5. Fail the release build if the Windows GigaSTT smoke test fails.

If the full server binary cannot be built cleanly on Windows, fallback order is:

1. Build a tiny Windows Rust sidecar around `gigastt-core` exposing only the operations Conversationaly needs.
2. Only as a development-only fallback, use the existing Windows UniFFI/Python wheel to validate output parity. Python is not a production packaging dependency.

## Sidecar lifecycle

Create a GigaSTT manager separate from `llama-helper` so the two processes cannot accidentally restart each other.

Responsibilities:

- Resolve bundled binary path using Tauri path/resource APIs.
- Resolve model directory under Conversationaly's app data, e.g. `models/gigastt/`.
- Start GigaSTT bound only to `127.0.0.1`.
- Start with long-file jobs enabled and pool size 1 for a single-user desktop workflow.
- Poll `/ready` with a bounded startup timeout.
- Expose typed status: `NotInstalled`, `Starting`, `Ready`, `Busy`, `Failed`.
- Gracefully terminate on app shutdown.
- Restart once on an unexpected process exit; never loop infinitely.
- Capture stderr/stdout into Conversationaly diagnostics without logging transcript text by default.

Suggested server mode:

```text
gigastt serve --enable-jobs --pool-size 1
```

Exact flags must be verified against the pinned GigaSTT version during implementation.

## Model management

Do not bundle the ~225 MB speech model into the application installer by default.

- Store GigaSTT models under the normal Conversationaly models area in a dedicated directory.
- Add a first-use download action with progress.
- Verify model availability before starting a transcription job.
- Surface disk-space/download errors in the UI.
- Keep the model version tied to the pinned GigaSTT version/manifest rather than guessing file names in Conversationaly.

## Audio preparation

The original meeting recording is immutable and remains the archival source.

GigaSTT accepts common audio containers but the finalized Conversationaly recording may use a container/extension that is not guaranteed to be accepted by the pinned GigaSTT build. Therefore post-processing uses a temporary normalized input when necessary.

Preferred implementation order:

1. Reuse Conversationaly's existing audio decoding/resampling path used by retranscription/import.
2. Write temporary PCM16 mono 16 kHz WAV from decoded samples using existing Rust audio/WAV utilities or a small focused writer.
3. Avoid bundling FFmpeg unless the repository's decoder cannot reliably consume the finalized meeting format on Windows.

Target temporary format:

```text
PCM signed 16-bit little-endian
1 channel
16,000 Hz
WAV
```

Temporary file location:

```text
<meeting>/.processing/gigastt-input.wav
```

On success it is deleted. On failure it may be retained until the user retries or the next cleanup pass, to avoid repeating expensive decode work.

## Post-recording transcription service

Add a focused use case separate from live transcription.

Proposed module shape:

```text
frontend/src-tauri/src/audio/post_transcription/
├── mod.rs
├── service.rs
├── gigastt_client.rs
├── gigastt_sidecar.rs
├── audio_prepare.rs
├── importer.rs
└── types.rs
```

### `service.rs`

Coordinates:

```text
meeting audio
  -> prepare audio
  -> ensure GigaSTT ready
  -> submit job
  -> stream/poll progress
  -> fetch result
  -> import transcript atomically
  -> emit completion event
```

The service must not know React/Tauri UI details beyond an event/progress sink interface.

### `gigastt_client.rs`

Typed local HTTP client for only the required GigaSTT endpoints:

- readiness,
- submit long-file job,
- get status/progress,
- cancel job,
- get final transcript.

Do not expose a generic HTTP client to the rest of the app.

### `importer.rs`

Converts GigaSTT output into Conversationaly's transcript row representation.

Rules:

- Prefer GigaSTT segment boundaries when available.
- Preserve absolute meeting-relative start/end times.
- Preserve confidence when the database supports it.
- Preserve `speaker` when GigaSTT diarization is enabled and reliable.
- Generate stable new row IDs for the replacement transcript.
- Replace all draft/live rows in one transaction after the complete result has been validated.
- Never delete the prior transcript before a valid replacement is ready.

## Transcript authority and states

A meeting transcript has a logical provenance/state:

```text
none -> draft_live -> final_gigastt
                  \-> failed_gigastt (draft preserved)
```

Existing database schema should be extended only if required to represent provenance/status. Prefer meeting-level metadata over touching every transcript row unless row-level provenance is genuinely needed.

Rules:

- Draft live transcript may be visible during recording.
- Summary generation after Stop should wait for the final GigaSTT result when auto-final-transcription is enabled.
- If GigaSTT fails, draft text is preserved and the user can retry, choose existing retranscription, or summarize the draft explicitly.
- A manual re-transcription never destroys the current final transcript until replacement succeeds.

## GigaSTT request defaults for Russian meetings

Initial defaults:

- model/head: default RNN-T accuracy-oriented head;
- VAD: enabled for pause-rich meetings;
- punctuation/casing: enabled;
- Russian ITN: enabled;
- segment output: enabled;
- word timestamps/confidence: request/store when available;
- diarization: off by default in the first production iteration; expose later after quality testing on real room recordings;
- pool size: 1.

Do not use GigaSTT streaming for the authoritative transcript.

## Live transcription behavior

Keep Conversationaly's current live STT implementation intact.

Add a setting:

```text
Live transcript during recording
[ ] Show draft transcript while recording
```

Target profile default: off.

When enabled, existing Conversationaly STT may produce a draft. Pressing Stop triggers GigaSTT and the final result replaces the draft after successful completion.

## Automatic post-processing / summaries

Do not redesign summary providers in v1.

Change orchestration so that when automatic GigaSTT finalization is enabled:

```text
Stop
 -> finalize audio
 -> GigaSTT final transcript
 -> optional diarization
 -> summary/post-processing provider
```

The summary layer consumes a stable final transcript. Existing local and API providers remain available.

Future structured post-processing can return a schema such as:

```json
{
  "summary": "...",
  "decisions": [],
  "action_items": [
    {
      "task": "...",
      "assignee": null,
      "deadline": null
    }
  ],
  "open_questions": [],
  "risks": []
}
```

This schema is deliberately not required for the first GigaSTT integration commit.

## UI changes

Reuse existing meeting history and transcript views.

After Stop show a clear stage state, for example:

```text
Recording saved
Preparing audio
Transcribing with GigaSTT — 46%
Finalizing transcript
Ready
```

Required actions:

- Cancel current GigaSTT job.
- Retry after failure.
- Re-transcribe a finished meeting with GigaSTT.
- Open/use existing summary after final transcript is ready.

The app remains usable while transcription runs; the Tauri UI thread must never block.

## Error handling and recovery

### Recording failure

Unchanged existing recovery path. GigaSTT never starts until recording finalization produces a usable audio file.

### Audio preparation failure

- Preserve original recording and existing transcript.
- Emit user-facing error with a diagnostic identifier.

### Sidecar/model failure

- Preserve original recording and current transcript.
- Stop automatic summary generation.
- Offer retry/download repair.

### Job failure/cancel

- No transcript replacement.
- Temporary processing file may remain for retry.
- GigaSTT job identifier is not treated as permanent meeting data after completion.

### Database replacement failure

- Transaction rollback leaves old transcript unchanged.
- Completed GigaSTT result may be retained in a temporary JSON file for recovery/debugging until next successful import.

## Security/privacy

- GigaSTT binds to loopback only.
- No external STT request is made in the default target profile.
- Transcript/audio content is not written to normal logs.
- API summary providers remain opt-in and retain their existing privacy semantics.
- Temporary files live inside application-controlled meeting/app-data directories.

## Testing strategy

### Unit tests

- GigaSTT response -> transcript row mapping.
- Word/segment timestamp conversion.
- Speaker/confidence mapping.
- Transcript atomic replacement semantics.
- Audio preparation output format.
- Sidecar state transitions.
- Job cancellation/error mapping.
- Summary gating until final transcript.

### Contract tests

Use a fake GigaSTT HTTP server to test:

- `/ready` startup sequence;
- job submit/progress/success;
- job failure;
- cancellation;
- malformed response;
- sidecar restart boundary.

### Windows native GigaSTT smoke test

On a Windows x86-64 CI runner:

1. Build pinned GigaSTT server binary.
2. Start it on loopback with a test model fixture/model cache.
3. Wait for readiness.
4. Transcribe a short Russian WAV fixture.
5. Assert non-empty expected-language text and valid timestamps.
6. Stop the process cleanly.

This is a release gate for bundling the sidecar.

### Integration tests

- Start meeting -> stop -> fake GigaSTT -> transcript replaced.
- Draft live transcript survives GigaSTT failure.
- Successful retry replaces draft atomically.
- Existing meeting can be re-transcribed.
- Summary trigger occurs after final transcript, not before.

### Manual Windows acceptance

- USB microphone selection persists.
- 60+ minute recording does not grow RAM with recording duration.
- Stop reliably finalizes audio.
- GigaSTT works while UI remains responsive.
- App restart preserves completed meeting/audio/transcript.
- Cancel/retry works.
- Installer includes all required sidecar/native runtime files.

## Implementation sequencing

1. Windows GigaSTT build/smoke spike and packaging proof.
2. Post-transcription domain types + fake client tests.
3. GigaSTT sidecar/client.
4. Audio preparation.
5. GigaSTT result importer + atomic replacement.
6. Manual re-transcription integration.
7. Automatic post-Stop orchestration.
8. Progress/cancel/retry UI.
9. Summary gating.
10. Windows packaging + end-to-end acceptance.
11. Optional diarization quality evaluation on real meeting audio.

## Success criteria

- A user can record a long room meeting from one external microphone on Windows without relying on real-time STT.
- Pressing Stop automatically produces a GigaSTT final transcript without blocking the UI.
- The original recording survives every STT/post-processing failure.
- A failed GigaSTT run never destroys an existing transcript.
- Existing Conversationaly meeting history and summary functionality continue to work.
- The packaged Windows build does not require Python, Docker, WSL, or a separately installed GigaSTT service.
- GigaSTT can be updated as an independently pinned component.
