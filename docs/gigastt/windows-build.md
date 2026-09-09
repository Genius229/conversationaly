# GigaSTT Windows build and smoke gate

## Pin

Initial pin: GigaSTT `v2.18.0` (source tag resolved to commit
`b4f7531b5e54af69e4766bb36c14cc6745e452b2`). The release supports Windows
x86-64 CPU and requires Rust 1.88+ and `protoc` at build time.

## Runtime contract

Start the bundled binary on loopback with jobs enabled and a single worker:

```powershell
gigastt.exe serve --host 127.0.0.1 --enable-jobs --pool-size 1 --batch-pool-size 1 --vad --punctuation --itn
```

The exact flags must be checked with `gigastt.exe serve --help` for the built
binary. Readiness is `GET /ready`. Submit `POST /v1/jobs` as
`application/octet-stream`; poll `GET /v1/jobs/{id}`; cancel with
`DELETE /v1/jobs/{id}`; fetch JSON from `GET /v1/jobs/{id}/result`.

## Release gate

Run `scripts/gigastt/windows-smoke.ps1` on a Windows x86-64 runner with a
Russian WAV fixture and a model cache. The script fails on startup timeout,
non-ready status, non-202 submit, missing job id, terminal failure, empty
result, or missing timing fields. Native Windows execution is not claimed by
this document until CI produces the attached command log and artifact list.

## Bundle inventory

The Tauri bundle must include `gigastt.exe` and every native DLL discovered by
the Windows smoke build. Python, Docker, WSL, and an externally installed
GigaSTT service are prohibited production dependencies.
