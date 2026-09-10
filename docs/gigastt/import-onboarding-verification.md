# GigaSTT import and optional first-run models

Approved follow-up, 2026-09-10. Baseline: `b363b09`.

Verified source: `b094433`. Windows app/installer CI
[34490125378](https://github.com/Genius229/conversationaly/actions/runs/34490125378)
**PASS**; contract matrix
[34490124748](https://github.com/Genius229/conversationaly/actions/runs/34490124748)
**PASS** (Ubuntu 112, Windows 106, plus 20 Windows stress repetitions).
Installer artifact: `10158154233`; downloaded hash and metadata in
`evidence/windows-import-onboarding-2026-09-10.json`.

## Import audio

The file picker and drag-and-drop import now use
`gigastt_import_audio({ sourcePath, title })`, not the legacy transcription
engine. GigaSTT models must be installed explicitly; the import dialog uses
the shared model card and never silently downloads models or falls back.

1. Validate the supported audio file without changing it.
2. Copy it read-only into a UUID-unique meeting archive and publish the copy
   atomically only after complete copy/flush validation.
3. Persist the meeting and required-final-transcript job in one transaction.
4. Register the existing managed GigaSTT job and return its standard snapshot.
5. Open the meeting page; the shared service prepares PCM16 mono 16 kHz WAV,
   transcribes, commits the final transcript and synchronizes archive exports.

`Audio imported` means that the archive/meeting are saved, **not** that recognition
has finished. Progress/cancel/retry are available on the meeting page. Imported
meetings obey the same Ready/refreshed-row and optional-diarization ordering
before automatic summary as recorded meetings.

After acceptance, failure/cancellation preserves the meeting and archived audio
for Retry. Validation/copy failure before persistence reports that no retry
meeting was created; the original remains untouched. An uncertain SQLite commit
outcome conservatively retains archived audio for recovery rather than risking
deleting audio referenced by a committed row.

Preparation is an owned runtime task, not owned by its IPC waiter. A cloneable
preparation lease survives validation-worker and waiter lifetimes. Application
close rejects new preparation, signals cancellation, waits for preparation to
drain, then joins the active transcription before DB/sidecar teardown. Copying
checks cancellation in bounded chunks. Read-only native audio validation uses
the existing decoder and may need to finish its current blocking operation;
ownership does not imply instantaneous interruption of every OS read.

## First launch

- Opening the wizard, resolving model recommendations, ordinary Continue,
  model-free completion and restart do not start model downloads.
- Initial model cards expose explicit Download buttons.
- **Set up models later** finishes model setup without installed models.
- If the user explicitly started a download, the different action
  **Continue downloads in background** explains that it will continue.
- macOS still proceeds to Permissions; model skip does not bypass that step.
- Native completion records observed readiness. Missing models remain
  `not_downloaded`, not falsely `downloaded`; selected model names are preserved.
- Platform/setup hydration is checked independently from model availability.
  Optional readiness errors do not force a model download or block completion.

## Verification

- Native harness: **112 passed**, Clippy `-D warnings`, scoped Rust formatting.
- Frontend focused contracts: **26 passed** (18 GigaSTT/import + 8 onboarding).
- TypeScript and Next production build: **PASS**, 12/12 pages.
- Existing unaffected checks: three Node scripts and eight Bun tests pass.
  The unchanged upstream `live-transcription-event-names.test.mjs` still reads
  an absent `stream_worker.rs`; see `catalog-ui-verification.md` for baseline
  evidence. No new test failure is accepted as that known issue.
- Independent onboarding, native lifecycle/transaction and registry reviews:
  **APPROVE** after their findings were fixed.
- Chromium built-page checks: **PASS**, zero page/console errors:
  first-run skip/restart, explicit downloads and WebView re-entry, retained Mac
  permissions, MP4-labelled import staging error/retry/Ready, obsolete file
  validation after closing/reopening, and the previous catalog scenarios.
- Desktop onboarding and minimum-width import-dialog screenshots inspected.

Important regressions include concurrent close wakeups, exit during copy,
dropping an IPC waiter before a real SQLite commit, retaining audio on an unknown
commit outcome, and a slow validation response for file A not overwriting a
subsequently selected file B. The latter was reproduced in real Chromium before
the epoch fix. The commit-drop test has an explicit pre-commit barrier.

These browser tests use the actual built UI with deterministic Tauri IPC,
**not real audio inference**. The Rust tests exercise the production staging,
registry, SQLite and service modules. Full native Windows compilation and
installer checks passed their separate CI gate; physical MP4 import acceptance,
USB/60-minute recording and graceful Windows sidecar shutdown remain separate.

```sh
cd frontend
node --experimental-strip-types --test src/lib/gigastt.test.ts tests/lib/onboarding-explicit-downloads.test.mjs
pnpm exec tsc --noEmit
pnpm run build
cd ..
PLAYWRIGHT_MODULE=/tmp/gigastt-ui-tools/node_modules/playwright \
  node scripts/gigastt/import-onboarding-ui-smoke.cjs
PLAYWRIGHT_MODULE=/tmp/gigastt-ui-tools/node_modules/playwright \
  node scripts/gigastt/catalog-ui-smoke.cjs
```

Install the temporary Playwright tool as described in `catalog-ui-verification.md`.
Both scripts serve the built UI on an ephemeral loopback port and retain runtime
artifacts under `/tmp`, never in Git.
