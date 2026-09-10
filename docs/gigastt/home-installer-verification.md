# Home transcription mode and Windows installer verification

## Scope

Approved follow-up to the `b094433` Windows build: make Home describe the actual
GigaSTT workflow before recording, and prevent partial Windows updates when
installed runtime files are locked or unwritable. No recognition/model profile
change; GigaSTT remains the pinned CPU/offline post-recording engine.

| Automatic final | Live preview | Home header |
| --- | --- | --- |
| On | Off | GigaSTT · After recording |
| On | On | GigaSTT · After recording / selected model · Live draft |
| Off | On | selected model · Live draft / GigaSTT · Manual final |
| Off | Off | Audio only / GigaSTT · Manual final |

Loading or failed settings reads show neutral status, not an invented Parakeet
selection. Empty/recording copy follows the same mode matrix. The UI subscribes
to successfully persisted setting changes and rejects stale initial reads.

## Confirmed baseline failure

The user screenshots report **Error opening file for writing**, not a missing
DLL loader error. All five named files (`DirectML.dll`, `MSVCP140.dll`,
`MSVCP140_1.dll`, `VCRUNTIME140.dll`, `VCRUNTIME140_1.dll`) are present in the old
NSIS payload with Archive, not ReadOnly, attributes. The exact lock/ACL condition
on the user's machine is not established from screenshots alone.

Real Windows regression run
[34500059042](https://github.com/Genius229/conversationaly/actions/runs/34500059042)
tested the shipped `b094433` installer (SHA256
`033f1c9740e0172dc4d8bcdfa7113db911cc2147c845086ce10de46caeecc6f1`):

- Fresh silent installation into a path with spaces: exit 0, 10.28 seconds.
- A test-owned external `gigastt.exe` held installed `DirectML.dll` open.
- Upgrade incorrectly returned exit 0 after 8.06 seconds.
- The installer terminated an unrelated external `conversationaly.exe`.
- The installed main executable changed before the DLL lock was released.

This is a genuine RED test against the old executable, not a source-string
assertion. File snapshots and process evidence are in that run's
`installer-acceptance-evidence` artifact; no audio/transcripts are included.

## Automated checks

Frontend checks: 31 GigaSTT/Home/onboarding Node tests, TypeScript, Next.js
production build (12 static pages), six Home browser scenarios, existing catalog
smoke and five import/onboarding scenarios passed. Browser tests exercise built
pages with mocked Tauri IPC; they do not prove native audio behavior.

Reproducible Home smoke:

```sh
cd frontend && pnpm build && cd ..
PLAYWRIGHT_MODULE=/path/to/playwright node scripts/gigastt/home-mode-ui-smoke.cjs
```

`tools/windows-installer-tests/acceptance.ps1` exercises actual installation,
locked update, passive/silent updater modes, successful retry, complete runtime
hashes, unrelated-process survival, separate model-data preservation, and
optional native cooperative application quit. Only test-created process objects
are killed during cleanup, never a process name. The full Windows build gates
its successful installer artifact on acceptance; failed candidates have a
separate diagnostics-only artifact name.

New installer GREEN evidence: pending completion of native build and acceptance.

## Verification boundary

The main window's X hides the application in the tray; it does not unload its
runtime. Cooperative **application** exit must refuse an active recording and
run existing cleanup. This is distinct from graceful GigaSTT **child** shutdown,
which remains an open release gate (current Windows child cleanup is force/reap).
Physical microphone/USB and 60-minute recording acceptance remain separate.
