# Conversationaly + GigaSTT Post-Recording Transcription Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Windows-first, post-recording GigaSTT transcription path that atomically replaces draft transcripts while preserving recording durability and existing summaries.

**Architecture:** Keep recording and live transcription unchanged. Add an isolated Rust post-transcription use case with a managed loopback GigaSTT sidecar, typed HTTP client, temporary PCM/WAV preparation, validated importer, and event-driven UI state; gate summaries on final transcript availability.

**Tech Stack:** Rust/Tauri, existing Conversationaly database/audio abstractions, local HTTP, GigaSTT 2.18.0 initially, Windows MSVC packaging, existing React UI.

**Spec:** `docs/superpowers/specs/2026-09-09-conversationaly-gigastt-design.md`

## Global Constraints

- Windows x86-64 is the primary packaged platform; production must not require Python, Docker, WSL, or an external GigaSTT service.
- GigaSTT is an isolated sidecar bound to `127.0.0.1`; pool size is 1 and restart is limited to one attempt.
- Authoritative transcription is post-recording full-file batch; GigaSTT streaming is out of scope.
- Original finalized audio is immutable; failed/cancelled jobs never delete or replace the current transcript.
- Default request profile: VAD, punctuation/casing, Russian ITN, segments, timestamps/confidence when available; diarization off.
- Temporary input is PCM16 LE mono 16 kHz WAV under `<meeting>/.processing/gigastt-input.wav` and is deleted after successful import.
- Do not log transcript/audio content by default.

## Source checkout prerequisite

Upstream `bykof/conversationaly` was cloned at `0359c12d492fbc6583229547989977ecd7aef723` into `conversationaly/`, with fork `Genius229/conversationaly` and implementation branch `feat/gigastt-post-transcription`. Preserve this existing application; do not scaffold a replacement.

### Task 1: Windows GigaSTT build and packaging spike

**Files:**
- Create: `docs/gigastt/windows-build.md`
- Create: `scripts/gigastt/windows-smoke.ps1`
- Modify: existing Tauri bundle config after source checkout is available

**Interfaces:**
- Produces a pinned GigaSTT binary/resource manifest and a repeatable Windows smoke command.

- [x] Verify the pinned 2.18.0 server/CLI flags from its source and build on Windows MSVC.
- [x] Run `gigastt.exe` on loopback with a Russian WAV fixture; assert readiness and non-empty timestamped output.
- [x] Record required DLL/native runtime files and fail the script when smoke output is invalid.
- [x] Commit the spike evidence and packaging manifest. Windows run `34430238245`; evidence in `docs/gigastt/evidence/windows-2026-09-10.json`.

### Task 2: Post-transcription domain types and fake-client contract tests

**Files:**
- Create: `frontend/src-tauri/src/audio/post_transcription/types.rs`
- Create: `frontend/src-tauri/src/audio/post_transcription/gigastt_client.rs`
- Create: `frontend/src-tauri/tests/post_transcription_client.rs`

**Interfaces:**
- `GigasttClient::ready`, `submit_job`, `get_job`, `cancel_job`, `get_result` are typed async methods.
- `PostTranscriptionState` includes `PreparingAudio`, `Starting`, `Transcribing { percent }`, `Finalizing`, `Ready`, `Failed`, `Cancelled`.

- [x] Write fake-server tests for ready, submit/progress/success, failure, cancellation, malformed JSON, and bounded HTTP errors.
- [x] Implement only the required endpoint DTOs and strict response validation.
- [x] Run the focused Rust tests and commit.

### Task 3: Managed GigaSTT sidecar

**Files:**
- Create: `frontend/src-tauri/src/audio/post_transcription/gigastt_sidecar.rs`
- Modify: `frontend/src-tauri/src/lib.rs` and Tauri resource/bundle configuration
- Test: `frontend/src-tauri/tests/gigastt_sidecar.rs`

- [x] Test state transitions, readiness timeout, bounded shutdown/reaping, and one-time unexpected-exit restart with a fake executable.
- [ ] Prove graceful Windows shutdown; current Windows implementation uses bounded force/reap, not graceful termination.
- [x] Resolve binary/model paths via Tauri path APIs; launch loopback server with verified flags and `--enable-jobs --pool-size 1` equivalent.
- [x] Capture stdout/stderr diagnostics without transcript payloads and expose typed status.
- [x] Run tests and commit.

### Task 4: Audio preparation and importer

**Files:**
- Create: `frontend/src-tauri/src/audio/post_transcription/audio_prepare.rs`
- Create: `frontend/src-tauri/src/audio/post_transcription/importer.rs`
- Create: `frontend/src-tauri/src/audio/post_transcription/service.rs`
- Test: `frontend/src-tauri/tests/post_transcription_import.rs`

- [x] Add failing tests for PCM16/mono/16 kHz WAV output, segment/word timestamp conversion, speaker/confidence mapping, malformed results, and atomic replacement rollback.
- [x] Reuse the existing retranscription decoder/resampler; write `.processing/gigastt-input.wav` and preserve it on failure. Native decoder adapter is source-wired; full desktop compile remains a separate gate.
- [x] Validate the complete result, generate stable new row IDs, and replace draft/live rows in one transaction only after validation. Preserve canonical top-level `result.text` in `meeting_transcript_metadata.result_metadata.text` for Task 5 summaries.
- [x] Implement the coordinator pipeline and progress sink, then run focused tests. Fourteen service tests cover cleanup/result retention and cancellation at the commit boundary; included in the integration commit.

### Task 5: Manual re-transcription and automatic Stop orchestration

**Files:**
- Modify: `frontend/src-tauri/src/audio/recording_manager.rs`, `recording_saver.rs`, `retranscription.rs`
- Modify: `frontend/src-tauri/src/lib.rs` command registration
- Create/modify: database migration/model for meeting-level transcript provenance
- Test: integration tests for stop, retry, failure preservation, and existing-meeting re-transcription

- [x] Add commands for start/cancel/retry/re-transcribe and ensure they never block the Tauri UI thread.
- [x] Chain finalized audio -> service -> final transcript before automatic summary when enabled; preserve draft on all failures.
- [x] Verify durable interrupted-job recovery and transaction rollback in headless tests; included in the integration commit. Real app restart acceptance remains Task 7.

### Task 6: Progress/cancel/retry UI and summary gating

**Files:**
- Modify: `frontend/src/components/Sidebar/SidebarProvider.tsx` and existing meeting/transcript views
- Create/modify: settings component for `Live transcript during recording` (default off)
- Test: frontend state/event tests

- [x] Render `Recording saved`, `Preparing audio`, percentage progress, `Finalizing transcript`, `Ready`, and actionable failure/cancel states.
- [x] Subscribe to Tauri events, keep the app usable during jobs, and expose retry/re-transcribe actions.
- [x] Ensure summaries consume only `final_gigastt` unless the user explicitly summarizes draft text.
- [x] Run frontend checks; included in the integration commit. Nine focused tests, TypeScript and Next build (12 pages) pass; independent UI review approved.

See `docs/gigastt/implementation-status.md` for current verification boundaries. Explicit model install/repair is also implemented (15 headless tests), as required by the spec. Physical UI/recording acceptance remains Task 7.

### Task 7: Windows end-to-end release gate

**Files:**
- Modify: Windows CI workflow, installer/resource manifest, release documentation
- Create: Windows E2E smoke harness and test fixture manifest

- [x] Build the pinned sidecar on a Windows x86-64 runner, start it with a model fixture, transcribe Russian WAV, and assert timestamps/language/non-empty text.
- [x] Verify installer includes executable and native runtime files but no Python dependency. Full Tauri/NSIS run `34463264744` (`119b77e`) PASS; executable and co-located DLL hashes verified after extraction.
- [ ] Execute manual acceptance: USB mic persistence, 60+ minute bounded-memory recording, responsive UI, restart recovery, cancel/retry, and summary ordering.
- [x] Publish automated build/installer evidence and commit the repeatable gate. `docs/gigastt/evidence/windows-desktop-2026-09-10.json`; this is not production release acceptance (graceful shutdown and physical Windows checks remain open).

## Verification matrix

- Rust unit/contract/integration tests for every post-transcription module.
- Frontend typecheck/lint/tests for progress and settings state.
- Windows native smoke test is mandatory for release packaging.
- Failure verification must demonstrate that original audio and prior transcript remain intact.
