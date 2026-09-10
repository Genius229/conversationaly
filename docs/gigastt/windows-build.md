# GigaSTT v2.18.0 native Windows build and inference gate

This gate builds and runs the native GigaSTT server on a GitHub-hosted Windows
x86-64 runner. It is a build/inference spike, not a signed release workflow and
not yet proof that a Conversationaly installer shuts the process down cleanly.

## Immutable inputs

The machine-readable pins live in
`scripts/gigastt/windows-resources.json`:

- source: `https://github.com/ekhodzitsky/gigastt.git`;
- tag: `v2.18.0`;
- commit: `b4f7531b5e54af69e4766bb36c14cc6745e452b2`;
- Rust: `1.88.0`;
- protobuf compiler: `30.2`;
- target: `x86_64-pc-windows-msvc` (CPU build, no CUDA feature);
- fixture: the public four-second, PCM16 mono 16 kHz Russian Golos fixture
  `crates/gigastt/tests/fixtures/golos_00.wav` from that pinned checkout,
  SHA-256
  `500d2e88a1634ee766fec46f83cb903f3a1c276b2cedb36f11db800a1c4c14d0`.

The fixture is upstream test data, not customer audio. Its reference sentence
is Russian, and the workflow fails if its hash changes.

The exact upstream-equivalent build command is:

```powershell
cargo build --release --locked `
  --target x86_64-pc-windows-msvc `
  -p gigastt
```

`scripts/gigastt/windows-build.ps1` refuses a different commit, tag,
toolchain, target, version output, or missing required CLI flags. The source's
tracked files must be clean before it invokes Cargo.

## Automated workflow

`.github/workflows/gigastt-windows.yml` runs on:

- every push to `feat/gigastt-post-transcription`;
- manual `workflow_dispatch`.

The entire job has a 90-minute deadline. It performs only the native sidecar
gate; it does not sign, publish, or attempt a full Tauri release.

The workflow:

1. fetches the exact GigaSTT commit and verifies that `v2.18.0` resolves to it;
2. verifies the public WAV hash;
3. builds the Windows MSVC CPU binary using the command above;
4. inventories the PE imports with Visual Studio `dumpbin`;
5. restores/downloads the pinned INT8 RNNT, RuPunct, and Silero VAD assets and
   verifies every SHA-256;
6. starts the staged executable in `--offline` mode on `127.0.0.1:9876`;
7. exercises the asynchronous jobs API and uploads diagnostics plus the staged
   native runtime.

No Python, Docker, or WSL runtime is used. The model download is an explicit CI
setup action; the subsequent server smoke runs offline.

## Verified v2.18.0 server command

The source defines `--offline` as a global option, so it must precede `serve`.
`--punctuation` and `--itn` take `on|off|auto` values; they are not bare boolean
flags.

```powershell
gigastt.exe --offline serve `
  --host 127.0.0.1 `
  --port 9876 `
  --model-dir <models\gigastt\2.18.0> `
  --model-variant rnnt `
  --pool-size 1 `
  --enable-jobs `
  --shutdown-drain-secs 10 `
  --vad `
  --vad-model-dir <models\gigastt\2.18.0\vad> `
  --punctuation on `
  --punct-model-dir <models\gigastt\2.18.0\punct> `
  --itn on
```

Do not add `--batch-pool-size 1` to this single-pool profile. GigaSTT clamps a
dedicated batch pool so that at least one interactive triplet remains; with a
total pool of one it cannot create that split. Omitting the flag makes jobs
share the one pool, which is the intended single-user desktop behavior.

The smoke submits:

```text
POST /v1/jobs?format=json&segments=true&word_timestamps=true&punctuation=true&itn=true&vad=true
GET  /v1/jobs/{id}
GET  /v1/jobs/{id}/result
```

It has separate bounded startup (180 seconds), job (180 seconds), HTTP request
(15 seconds), and child-reaping (20 seconds) deadlines. It fails on malformed
status/progress, non-202 submission, a missing job ID, a non-successful terminal
state, empty/non-Cyrillic text, missing words/segments, invalid confidence, or
negative/reversed/non-monotonic timestamps. `/health` must report GigaSTT
2.18.0, RNNT, punctuation, and ITN; the server log must prove VAD loaded.
Transcript text is not printed or saved. The evidence stores only counts,
timings, and its SHA-256.

## Runtime DLL inventory and Tauri staging

The build script stages files at:

```text
frontend/src-tauri/binaries/gigastt-runtime/
├── gigastt.exe
├── *.dll                         # only when actually required
├── dumpbin-dependents.txt
└── runtime-inventory.json
```

Every staged PE must be x64. Imports are resolved fail-closed:

- genuine Windows components and API-set contracts are accepted only through
  an explicit allowlist and must exist in `System32`;
- imported Visual C++ redistributable DLLs (`vcruntime*`, `msvcp*`, `concrt*`,
  `msvcr*`) are copied app-local from the x64 Visual Studio redistributable;
- an unknown or unresolved import fails the build;
- app-local DLLs are inspected recursively;
- `.py`, `.pyc`, `.pyd`, and Python executables fail the package check.

`runtime-inventory.json` records the binary/DLL hashes, import resolution,
source pin, toolchains, and Cargo command. This generated file is authoritative
for the particular Windows build; do not guess a static DLL list from Linux.

The ignored staging directory is mapped to `$RESOURCE/gigastt/` by the opt-in
overlay:

```text
frontend/src-tauri/tauri.gigastt.windows.conf.json
```

It is deliberately **not** named `tauri.windows.conf.json`: ordinary Windows
development/build workflows do not stage GigaSTT yet and must not break merely
because this spike exists. After a successful native artifact has been copied
to the staging directory, opt in from `frontend/` with:

```powershell
pnpm tauri build --config src-tauri/tauri.gigastt.windows.conf.json `
  --target x86_64-pc-windows-msvc
```

The merged resource layout is
`resource_dir()/gigastt/gigastt.exe`, with required DLLs beside the executable.
It is a resource directory rather than `externalBin`, because Tauri's sidecar
renaming does not provide a useful way to keep an audited DLL set alongside the
executable.

## Shutdown boundary and release status

GigaSTT v2.18.0 waits on `tokio::signal::ctrl_c()` on Windows. The hosted
Actions PowerShell and the server share a console; broadcasting
`GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0)` would also signal unrelated runner
processes. The CI harness therefore uses bounded `Kill(true)` **only to reap the
finished smoke child** and records `shutdown=forced-reaped` and
`graceful_shutdown=NOT_VERIFIED`. It does not call this graceful.

A real Tauri manager can request a supported console signal only if it launches
the child in an isolated console/process group with verified signal handling,
or upstream adds a local authenticated shutdown endpoint. Clean manager/app
shutdown remains a separate Windows release-acceptance gate.

Native Windows build and inference success must not be claimed until the actual
GitHub Actions run is green. Even a green native spike does not by itself prove
installer inclusion, app lifecycle, cancel/retry, USB microphone behavior, or
the 60-minute manual acceptance checks.
