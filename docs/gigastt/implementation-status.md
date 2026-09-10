# GigaSTT integration — verification ledger

Updated: 2026-09-10. Branch: `feat/gigastt-post-transcription`.

**Automated integration gates passed:** full Windows Tauri/NSIS build, pinned
GigaSTT native inference smoke, Windows/Ubuntu contract matrix, Clippy and
frontend checks. UI/backend reviews approved. Remaining acceptance boundaries
are listed below; this is an unsigned development build, not a production release.

Catalog usability follow-up: GigaSTT download/repair is now available directly
in **Settings → Transcription → On-device models**, with shared meeting-panel
state, search and Installed only. Local UI/browser checks and independent review
passed; an updated Windows installer is pending. See `catalog-ui-verification.md`.

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
- Temporary PCM16 mono 16 kHz WAV preparation through
  the existing decoder adapter, streaming file upload, and atomic SQLite
  importer. A failed/cancelled import preserves old rows and provenance;
  normalized temp files are replaced atomically without modifying the archive.
- Importer stores exact canonical `result.text`, confidence, segments and
  word timings in meeting-level metadata. Segment rows retain their boundaries;
  no-segment fallback retains the full final text rather than raw word tokens.
  Stable IDs hash the full result once, not once for each row.
- Complete coordinator: verify meeting identity, prepare audio, await sidecar,
  submit/poll with a hard deadline, retain validated result JSON, atomically
  import, then clean up. Import failure retains the result; post-commit cleanup
  failure is reported as `cleanup_pending`, not as a failed transcription.
- Explicit model install/repair for all eight pinned RNNT, punctuation and VAD
  files, with SHA256 checks, bounded streaming downloads, cancellation and
  throttled progress. No model download happens automatically at startup.
- Native start/cancel/retry commands own and join background tasks. Durable job
  state recovers interrupted runs; legacy retranscription clears Giga authority
  atomically. Recording, transcription, diarization and model installation share
  the exclusion guard where their resources overlap.
- Stop hands off only the exact nonempty audio path returned by successful
  finalization inside the meeting directory. A stale file from a previous save
  cannot authorize automatic transcription.
- Backend summary gating reads canonical final text and optional DB-derived
  speaker annotations in one transaction. Using a draft requires explicit
  `allowDraft`; failed required finalization does not silently fall back.
- Settings default to automatic post-transcription on and live preview off.
  Disabled preview drains audio without loading a live ASR model. Null system
  input means microphone-only; null mic independently resolves the default mic.
- Meeting UI supports progress, cancel/retry, model installation and explicit
  draft summaries. Final rows must refresh successfully before automatic work;
  optional diarization must finish before automatic summary. Refresh failures
  preserve the displayed transcript and can be retried without rerunning STT.

## Commands and boundaries

```sh
cargo test --manifest-path tools/gigastt-contract-tests/Cargo.toml --locked --features gigastt-test-fixtures
cargo clippy --manifest-path tools/gigastt-contract-tests/Cargo.toml --locked --features gigastt-test-fixtures --all-targets -- -D warnings
cd frontend
node --experimental-strip-types src/lib/gigastt.test.ts
pnpm exec tsc --noEmit
pnpm run build
cd ..
git diff --check
```

Local Rust result: **98 passed, 0 failed** (15 client + 15 process lifecycle +
9 audio preparation + 10 importer + 14 service + 15 models + 6 durable job state
+ 2 preview + 1 finalization + 1 environment + 10 fake transport regressions),
Clippy exit 0.
Tests require permission to open loopback sockets; a socket-restricted sandbox
is not a supported execution environment for these contract tests.
Tests include real loopback HTTP and owned fake subprocesses,
not actual GigaSTT model inference. Independent review and CI results are
recorded as they finish; these local results do not certify the Windows app.

Frontend verification: **12 state/contract tests passed**, TypeScript exit 0,
Next production build exit 0 (12/12 pages). These are not a real Tauri UI or
physical microphone acceptance run. The catalog follow-up also passes Chromium
mock-IPC checks; its report explicitly identifies one unchanged stale upstream
event-name test separately from the passing checks.

Full native check attempted with `cargo check -p conversationaly --offline`:
blocked in `alsa-sys` because this Linux host lacks `alsa.pc`. The Tauri
adapter and desktop dependency graph are therefore not certified by this
headless test result. They are compiled by the successful full Windows CI below.

## Windows gates

- `GigaSTT contracts`: headless test/Clippy matrix on Ubuntu and Windows.
- `GigaSTT native Windows gate`: pinned MSVC build, public Russian WAV
  fixture, hash-checked models, offline jobs inference and runtime inventory.
- `GigaSTT desktop Windows check`: calls the pinned native build, verifies its
  same-run artifact, builds the real CPU llama-helper and full Tauri application,
  then extracts the unsigned development NSIS installer and verifies co-located
  GigaSTT executable/DLL hashes. Run `34463264744` (`119b77e`) **PASS**, including
  full native Tauri compilation, CPU llama-helper, portable ggml flags, NSIS
  extraction/runtime hash verification and installer upload. Unsigned development
  artifacts are not production releases.
  The development overlay uses a separate product name and identifier, so its
  install directory and AppData/DB do not replace the regular application.
- The packaging overlay is **opt-in**; ordinary Windows builds do not depend
  on an absent staged GigaSTT executable.
- **Graceful Windows shutdown remains a release limitation.** The desktop
  manager currently uses bounded force/reap on Windows; Unix uses SIGTERM
  with bounded fallback. Do not label forced termination as graceful.
- Automated installer payload verification is proven by CI. Installed-app
  interaction and physical-device acceptance are separate, still-open checks.

Full desktop build and packaging **PASS**:
https://github.com/Genius229/conversationaly/actions/runs/34463264744
(`119b77e`). Installer artifact: `10148002744`,
`Conversationaly GigaSTT Dev_1.4.1_x64-setup.exe` (62,198,456 bytes).
Downloaded installer SHA256:
`4023143dae30b4f6539f12b0e91ca98658cea315a190b151499424ddc2d7a64c`.
Metadata-only evidence: `evidence/windows-desktop-2026-09-10.json`.
This installer predates the catalog usability follow-up; it remains valid
evidence for native integration, not the updated Settings card. Native spike in
this run: 4 s audio, startup 3.087 s, job 1.57 s.

Native build/inference **PASS**:
https://github.com/Genius229/conversationaly/actions/runs/34430238245
(`7e61e7f`). Four-second Russian fixture: six Cyrillic words, one segment,
valid timestamps, punctuation and ITN enabled. Compact evidence (no transcript
or audio) is in `evidence/windows-2026-09-10.json`. This remains a sidecar
spike, not full installer/graceful-shutdown acceptance.

Contract CI **PASS** on both Windows and Ubuntu:
https://github.com/Genius229/conversationaly/actions/runs/34468472154
(`b9f26c7`). Ubuntu: **98 passed**, Windows: **92 passed**, plus **20 consecutive
Windows readiness-timeout stress passes**; Clippy passed on both platforms.
Platform counts differ because six tests are Unix-only.
Evidence: `evidence/contracts-2026-09-10.json`.

Earlier CI history: `ce82663` fixed the Windows canonical/short-path assertion
exposed by run `34432100449`. Integration run `34463264459` (`119b77e`)
passed Ubuntu but exposed a Windows test-fixture race: a cancelled readiness
probe reset its socket, and fake-server `expect()` incorrectly crashed the
process. The test-only fix tolerates disconnected writes, with a deterministic
BrokenPipe regression. Rerun `34464996947` confirmed all Windows lifecycle tests
pass, then exposed a model-progress test's fixed wall-time assumption: Windows
timer granularity stretched 120 short sleeps, allowing more legitimate 100 ms
updates. That assertion now uses actual elapsed time and checks intermediate
event spacing, while retaining exact initial/final byte assertions. Production
supervisor/installer behavior is unchanged. Run `34466298943` showed the
write-only disconnect fix was incomplete: request-header/body reads and queued
Windows accepts also need disconnect handling. The fake now handles expected
transport errors explicitly, still fails malformed/unexpected requests, and
records sanitized stage/kind diagnostics. Ten deterministic fixture tests cover
these boundaries, including full 8,193-byte body preservation. The 98-test Linux
suite and Clippy pass; CI now collects all suites with `--no-fail-fast` and runs
the Windows readiness-timeout regression 20 consecutive times. Final run
`34468472154` passed all of these checks.

Initial run `34429102821` failed workflow validation because `runner.temp`
is not permitted at job env scope; fixed with step initialization. Run
`34429289297` compiled the native binary in 8m15s but exposed a packaging
check bug: Windows API-set imports are virtual loader contracts, not always
DLL files in System32. Fixed that classification and a PowerShell scalar
OrderedDictionary `.Count` issue (verified locally for zero/one/two entries).
The GigaSTT workflows pass
`actionlint v1.7.7` (shellcheck/pyflakes disabled; PowerShell/native execution
is validated by CI, not by that static check). PowerShell 7.5.2 also parses
both scripts locally without AST errors.

## Remaining approved implementation

1. Close the graceful Windows shutdown release limitation with a
   supported/proven mechanism (native build/inference smoke already passes).
2. Exercise the Rust model installer against the real pinned model host; current
   installer tests use fake HTTP, while the proven Windows smoke uses PowerShell.
3. Execute real Windows microphone/USB persistence, 60+ minute bounded-memory
   recording, responsive UI, restart/cancel/retry and summary ordering acceptance.
   Keep upstream recording durability intact. Checklist: `windows-acceptance.md`.

## Explicit local trust boundary

The pinned GigaSTT CLI does not offer socket inheritance or an authenticated
readiness rendezvous. Port preflight plus an owned child avoids reusing an
already listening service, but is not a cryptographic ownership proof: a
same-user local process could race the bind between preflight and child
startup. The current design assumes a trusted local host. Hardening against
adversarial local port races requires a separate supported IPC/authentication
mechanism and is not claimed by these lifecycle tests.
