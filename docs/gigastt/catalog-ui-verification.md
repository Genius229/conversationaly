# GigaSTT catalog usability verification

Approved follow-up, 2026-09-10. Baseline: `4f8896b`.

## Change

Settings → Transcription → On-device models now includes a **GigaSTT 2.18.0**
card labelled **After recording · Offline**. It participates in search and
Installed only, and exposes download/repair/cancel/progress without first
creating a meeting. Settings and the existing meeting panel share the same
model state and install-command coordination. Downloading GigaSTT does not
select a legacy live model or change its provider.

Eight files are labelled `required` until verification succeeds, then `verified`.
No unmeasured total download size or WER/speed claim is added. Catalog controls
wrap within the application's supported 720-pixel minimum window width.

No native recognition, model pin, audio conversion or direct-import behavior
was changed. The user reported that basic Windows GigaSTT operation worked
before this UI change; that report is not the full physical acceptance matrix.

## Fresh verification

- GigaSTT frontend contracts: **12 passed**.
- TypeScript: **exit 0**.
- Next production build: **exit 0**, 12/12 pages.
- Rust headless harness: **98 passed**; Clippy `-D warnings`: **exit 0**.
- Existing frontend tests: **3 Node scripts and 8 Bun tests passed**.
- One existing Node script, `live-transcription-event-names.test.mjs`, still
  fails because it reads absent `audio/transcription/stream_worker.rs`.
  Its blob is unchanged (`e9dd5913c3341780df9b9818b5e8ef82b35980a1`) and that
  target file is absent in baseline `4f8896b` too. This unrelated stale test
  was not repaired as a side effect of the catalog change.
- Independent frontend review: **APPROVE**.
- Chromium browser integration: **PASS**, no uncaught or console errors,
  desktop 1280px and minimum-width 720px screenshots inspected.

Browser assertions cover initial missing state, search inclusion/exclusion,
Installed only, download/cancel/failure/retry, final verification, persistence
across Settings/meeting navigation, no implicit installs, no legacy-provider
writes and visible sorting controls at minimum width. Baseline RED reproduced
the absent catalog card; a geometry assertion also reproduced the clipped
sort control before the scoped wrap fix.

## Reproduce the browser check

The script serves `frontend/out` itself on an ephemeral loopback port and uses
a deterministic Tauri IPC fixture. It runs the **real built pages**, not a
duplicate UI, but does **not** exercise native downloads or recognition.

```sh
cd frontend
pnpm run build
cd ..
npm install --prefix /tmp/gigastt-ui-tools --no-audit --no-fund playwright
PLAYWRIGHT_MODULE=/tmp/gigastt-ui-tools/node_modules/playwright \
  CHROMIUM_PATH=/usr/bin/chromium \
  node scripts/gigastt/catalog-ui-smoke.cjs
```

Optional variables: `CATALOG_UI_ARTIFACTS` selects the screenshot directory;
`CATALOG_UI_BASE_URL` uses an already running frontend instead of serving the
static export. Default screenshots and console results remain under
`/tmp/gigastt-catalog-ui`, not in Git. The Windows workflow also runs the focused
frontend contract tests before building the application.
