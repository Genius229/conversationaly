# GigaSTT integration — verification ledger

Updated: 2026-09-10. Branch: `feat/gigastt-post-transcription`.

## Implemented and locally exercised

- Typed v2.18 jobs client. Loopback-only HTTP, no proxy/redirects, bounded body
  and request timeouts, redacted error text, validated identity and nested timings.
- Owned sidecar supervisor. Serialized startup/shutdown, bounded readiness,
  one unexpected-exit restart, occupied-port preflight, terminal app
  close, bounded metadata-only diagnostics, child termination/reaping.
- Tauri adapter resolves bundled executable from `resource_dir/gigastt/` and
  models from `app_data_dir/models/gigastt/2.18.0/`. The supervisor serves
  `--offline`: startup does not silently fetch models or submit external STT.
- Test-only fake executable is gated from normal desktop builds behind
  `gigastt-test-fixtures`. The independent headless harness compiles the same
  production modules, without GTK or inference model dependencies.
- Task 4 foundations: temporary PCM16 mono 16 kHz WAV preparation through
  the existing decoder adapter, streaming file upload, and atomic SQLite
  importer. A failed/cancelled import preserves old rows and provenance;
  normalized temp files are replaced atomically without modifying the archive.
- Importer stores exact canonical `result.text`, confidence, segments and
  word timings in meeting-level metadata. Segment rows retain their boundaries;
  no-segment fallback retains the full final text rather than raw word tokens.
  Stable IDs hash the full result once, not once for each row.

## Commands and boundaries

```sh
cargo test --manifest-path tools/gigastt-contract-tests/Cargo.toml --locked
cargo clippy --manifest-path tools/gigastt-contract-tests/Cargo.toml --locked --all-targets -- -D warnings
git diff --check
```

Local result: **50 passed, 0 failed** (15 client + 15 process lifecycle +
9 audio preparation + 10 importer + 1 cleanup regression),
Clippy exit 0. Tests include real loopback HTTP and owned fake subprocesses,
not actual GigaSTT model inference. Independent review and CI results are
recorded as they finish; these local results do not certify the Windows app.

Full native check attempted with `cargo check -p conversationaly --offline`:
blocked in `alsa-sys` because this Linux host lacks `alsa.pc`. The Tauri
adapter and desktop dependency graph are therefore not certified by this
headless test result.

## Windows gates

- `GigaSTT contracts`: headless test/Clippy matrix on Ubuntu and Windows.
- `GigaSTT native Windows gate`: pinned MSVC build, public Russian WAV
  fixture, hash-checked models, offline jobs inference and runtime inventory.
- The packaging overlay is **opt-in**; ordinary Windows builds do not depend
  on an absent staged GigaSTT executable.
- **Graceful Windows shutdown remains a release limitation.** The desktop
  manager currently uses bounded force/reap on Windows; Unix uses SIGTERM
  with bounded fallback. Do not label forced termination as graceful.
- No native Windows PASS or installer acceptance is claimed before actual
  CI/hardware evidence. The workflow being present is not that evidence.

Native build/inference **PASS**:
https://github.com/Genius229/conversationaly/actions/runs/34430238245
(`7e61e7f`). Four-second Russian fixture: six Cyrillic words, one segment,
valid timestamps, punctuation and ITN enabled. Compact evidence (no transcript
or audio) is in `evidence/windows-2026-09-10.json`. This remains a sidecar
spike, not full installer/graceful-shutdown acceptance.

Contract CI **PASS** on both Windows and Ubuntu:
https://github.com/Genius229/conversationaly/actions/runs/34429705913
(`33667f9`).

Initial run `34429102821` failed workflow validation because `runner.temp`
is not permitted at job env scope; fixed with step initialization. Run
`34429289297` compiled the native binary in 8m15s but exposed a packaging
check bug: Windows API-set imports are virtual loader contracts, not always
DLL files in System32. Fixed that classification and a PowerShell scalar
OrderedDictionary `.Count` issue (verified locally for zero/one/two entries).
Both new workflows now pass
`actionlint v1.7.7` (shellcheck/pyflakes disabled; PowerShell/native execution
is validated by CI, not by that static check). PowerShell 7.5.2 also parses
both scripts locally without AST errors.

## Remaining approved implementation

1. Close the graceful Windows shutdown release limitation with a
   supported/proven mechanism (native build/inference smoke already passes).
2. Explicit first-use model download/repair action with progress and errors,
   covering main RNNT **and** punctuation/VAD side models.
3. Finish Task 4: connect the tested audio/client/importer components in the
   post-transcription service; retain result JSON on import failure and remove
   temporary files only after successful commit. **Service is not implemented yet.**
4. Task 5: manual/retry commands, Stop orchestration, meeting provenance and
   backend summary gating.
5. Task 6: settings/default-off live preview, progress/cancel/retry UI.
6. Task 7: full Windows app/installer gate and real microphone/long-recording
   acceptance. Keep upstream recording durability intact.

The application does not call the importer automatically yet: the service,
recording integration and UI are still pending. Task 5's canonical summary
read path must use `meeting_transcript_metadata.result_metadata.text` for
`final_gigastt` (segment text can differ from the full ITN/punctuated text).

## Explicit local trust boundary

The pinned GigaSTT CLI does not offer socket inheritance or an authenticated
readiness rendezvous. Port preflight plus an owned child avoids reusing an
already listening service, but is not a cryptographic ownership proof: a
same-user local process could race the bind between preflight and child
startup. The current design assumes a trusted local host. Hardening against
adversarial local port races requires a separate supported IPC/authentication
mechanism and is not claimed by these lifecycle tests.
