# Import codec fallback

## Report and scope

The user's Windows recording decoded successfully and produced six transcript
segments. An imported OGG/Opus failed validation with `unsupported audio codec`;
an imported M4A failed audio preparation before recognition. That M4A's exact
codec cannot be established without the file. The installer and model selection
are not the failing layer.

The shared decoder previously preconverted only MKV, WebM and WMA with FFmpeg.
Other containers were assumed to have a Symphonia-compatible codec. They now
try native decoding and, if probing/codec creation/packet decoding or complete
sample extraction fails, make one FFmpeg attempt. The three existing forced
FFmpeg formats retain their single-conversion path; no recursive retries occur.

Import validation and post-recording/imported-audio preparation use this same
decoder. Pure validation and codec code are separately compiled by the headless
test harness, rather than copied or replaced with fake decoders. Desktop public
entry points and the existing mono/16 kHz normalization remain unchanged.

Conversion uses an OS-temporary PCM WAV, not a file beside the original (which
may be read-only). Source rate and channel count remain available to existing
decoder consumers; GigaSTT's existing preparation converts to mono/16 kHz.
RAII deletes the temporary WAV on normal success/error unwinding. FFmpeg is
looked up locally without invoking its installer; codec fallback does not start
a download. FFmpeg stderr/metadata are not copied into application logs.
FFmpeg input protocols are restricted to local files, so an imported playlist
cannot fetch HTTP/network segments. Invalid oversized frame-count metadata
disables only optional progress estimation instead of overflowing.

## Executable checks

```sh
python3 tools/audio-codec-contract-tests/check-pins.py
cargo test --manifest-path tools/audio-codec-contract-tests/Cargo.toml --locked -- --test-threads=1
cargo clippy --manifest-path tools/audio-codec-contract-tests/Cargo.toml --locked --all-targets -- -D warnings
```

The harness compiles the actual production codec, FFmpeg locator and import
validator. Its Symphonia features and 15 codec/locator package pins must match
the desktop lockfile. The real FFmpeg generates synthetic tones at runtime:

- OGG/Opus and M4A/ALAC: first prove the native registry rejects the codec, then
  exercise successful validation and decoding through the shared entry point.
- M4A/AAC, MP4/AAC and PCM WAV: normal-format regressions.
- Unicode/spaces/canonical paths (including Windows extended paths), read-only
  originals, source SHA256 preservation and temporary-WAV cleanup.
- Invalid media: explicit failure without source mutation or content in errors.
- Codec unit checks: one-shot retry policy, FFmpeg arguments, temporary-file
  ownership and rejection of incomplete native packet decoding.

These fixtures contain no user audio or transcripts and are not committed.
ALAC is a representative unsupported M4A codec, not a claim about the original
user file. The full Windows build runs this suite with its actual bundled FFmpeg
before publishing the installer, in addition to the nine installation tests.

Local real-media and codec checks: 10/10 PASS; Clippy PASS. Existing GigaSTT
headless regressions: 115/115 PASS. Linux codec CI
[34536327944](https://github.com/Genius229/conversationaly/actions/runs/34536327944)
passed with actual FFmpeg. Full Windows build
[34536182311](https://github.com/Genius229/conversationaly/actions/runs/34536182311)
**PASS**: 10 codec tests against the bundled Windows FFmpeg, native inference
smoke, desktop/NSIS build, frontend contracts and all nine installer cases.

[Download artifact 10176121731](https://github.com/Genius229/conversationaly/actions/runs/34536182311/artifacts/10176121731)
contains the updated Windows installer (`6e3f417`). SHA256:
`c82b2731373d3c8ab0a46d553a3d2ed126b9b9377162522fe7d36ef5629ff0f6`.
Metadata-only evidence: `evidence/windows-import-codecs-2026-09-11.json`.
