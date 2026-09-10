# GigaSTT headless verification

This independent Cargo workspace compiles the **original** production Rust
modules by path and runs the same integration test files as the desktop app.
It does not link GTK, Tauri, transcribe.cpp, or llama.cpp and does not run their
build scripts. No test-only copies of the client or supervisor are maintained.

```sh
cargo test --manifest-path tools/gigastt-contract-tests/Cargo.toml --locked
cargo clippy --manifest-path tools/gigastt-contract-tests/Cargo.toml --locked --all-targets -- -D warnings
```

Tests open loopback sockets and launch fixture subprocesses. Sandboxes must
permit those operations. Native Tauri integration, Windows packaging, model
inference and microphone capture require separate checks: passing this suite
does not imply a passing desktop build or Windows acceptance.
