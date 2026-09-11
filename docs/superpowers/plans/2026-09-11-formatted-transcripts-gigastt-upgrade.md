# Formatted transcripts and GigaSTT 2.21 public Windows build

> **For agentic workers:** Use subagent-driven-development with bounded independent reviews.

**Goal:** Preserve GigaSTT punctuation/casing in displayed/exported transcript rows, upgrade to verified stable GigaSTT 2.21.0 and publish a directly downloadable Windows EXE.

**Architecture:** Top-level `result.text` is authoritative. Align it monotonically to existing segment times with bounded work and conservative handling of ITN rewrites. Runtime version and model-storage layout are separate so the same verified models remain usable.

**Spec:** Approved in this thread. The user explicitly cancelled native GigaSTT diarization after its duration limitation was explained. Do not include a toggle, model pack, migration or changed diarization behavior.

## Global constraints

- Preserve original audio, prior transcripts on error/cancel, existing model files and the GigaSTT Dev application/data identity.
- Do not invent wording, punctuation, speaker IDs or timestamps. Preserve complete canonical final text; uncertain attribution must not invent a speaker.
- Existing manually edited rows must not be silently overwritten. Existing recordings can be explicitly re-transcribed to apply formatting.
- Keep CPU, loopback-only, offline, pool 1 and file-window concurrency 1. No Python/Docker/WSL production dependencies or implicit model downloads.
- Keep all existing diarization behavior unchanged; GigaSTT requests still use diarization=false.
- Runtime pin: stable **2.21.0**, commit `5289e8a6b3a67217711e6b5cba6c2f19dec3c5eb`, Rust **1.94.0**. No moving branch references.
- The eight base model assets are unchanged. Keep `models/gigastt/2.18.0` and its matching model cache; do not force re-download.
- Public binary is a manually installed preview until separately documented release gates are closed. Do not claim automatic-updater or graceful child-shutdown work is complete.

## Tasks

- [x] Verify latest stable source, Windows/CPU API/flag compatibility and identical model hashes. Official archive lacks DLLs, so retain the full source-build/runtime-inventory pipeline.
- [x] Implement/test canonical text alignment for punctuation, case, ITN, repeated words, deleted fillers and ambiguous cross-segment rewrites; preserve timed bounds and conservative speaker labels.
- [x] Remove all cancelled native-diarization work; retain only version/asset-layout/serial-window updates.
- [x] Update build pins, application version 1.4.2 and truthful UI version labels. Include pinned upstream LICENSE/NOTICE in the installer.
- [x] Independently review scoped code; run complete headless/Clippy, frontend/type/build/browser, codec and Windows native/installer gates.
- [x] Publish a public GitHub prerelease with EXE and SHA256, verify anonymous direct download and record immutable release/build provenance.
- [x] Finalize docs and clean commit/push; never commit generated audio, transcripts or binary artifacts.
