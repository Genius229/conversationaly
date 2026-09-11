# Formatted transcripts and GigaSTT 2.21.0

## Scope

Application version: **1.4.2**. GigaSTT runtime: **2.21.0**, pinned to
`5289e8a6b3a67217711e6b5cba6c2f19dec3c5eb` with Rust 1.94.0.
The Windows source-build and DLL dependency inventory are retained; the
upstream archive alone is not a replacement for the complete runtime payload.
Upstream LICENSE and NOTICE are included and verified as installer resources.

Native GigaSTT diarization was explicitly cancelled by the user. No new
diarization toggle, model download or migration is included. Existing
Conversationaly diarization behavior is unchanged.

## Formatting correction

GigaSTT applies punctuation, casing and ITN to top-level `result.text`, while
its segment/word text remains raw. The importer now aligns that canonical text
to timed rows instead of displaying the raw segment text. It uses bounded
32-token lookahead with forward-context matching; ambiguous rewritten spans
are merged conservatively and conflicting speaker labels cleared. It never
generates punctuation or wording itself.

Existing transcript rows are not silently rewritten, preserving manual edits.
Use **Re-transcribe with GigaSTT** on an existing recording to apply the fix.
The same corrected rows feed display and export.

## Compatibility

The eight model assets are unchanged between the pinned versions. Their
storage layout remains `models/gigastt/2.18.0`; this is an asset-layout version,
not the executable version. Existing verified downloads are reused.
CPU execution, offline mode, loopback binding, pool size 1 and explicit
file-window concurrency 1 remain in place. The application/data identity
remains `com.conversationaly.gigastt-dev`.

## Verification

Local verification before the Windows build: 124 headless Rust tests,
Clippy with warnings denied, 31 focused frontend tests, TypeScript and Next
production build, catalog browser smoke and six Home scenarios passed.
Importer coverage includes repeated anchors, ITN spanning segment boundaries
and a 2,048-segment regression. Independent importer review approved.

The Windows native gate additionally feeds a real jobs response into
`verify-gigastt-format`, which executes the production importer and asserts
visible rows reconstruct the complete canonical text with punctuation/case.
It prints only counts and booleans. Raw diagnostic audio/transcripts are not
committed or published.

The official fixture starts with a number phrase. Its ITN result legitimately
contains a leading number and no uppercase letters; the initial native gate
incorrectly rejected this despite retaining punctuation and exact canonical
text. The corrected gate retains the original ITN check and runs a second
native pass with per-request ITN disabled, explicitly requiring uppercase
letters there. Two verifier regressions cover both cases.

Windows stress also exposed a test-only serial HTTP fake blocking a known-job
DELETE behind an abandoned GET header read. An initial 40 ms scoped read limit
proved too short on Windows and was removed. The final fake reads connections
concurrently with its original 2-second read limit, then handles fully read
requests/state sequentially. A deterministic partial-header/GET/DELETE
regression fails against the old fake and passes against the corrected fake.
The reader also explicitly selects blocking mode: Winsock `accept` inherits
the nonblocking listener property, unlike Linux. A separate regression models
that inherited mode on Linux and verifies delayed headers do not produce
`WouldBlock`/transport failures. Both corrections are limited to the fake.
All timeout, remote cancellation, original-transcript and Ready-state
assertions remain. Production service limits/logic are unchanged.

## Final public artifact

- Full Windows native/desktop/NSIS run: **34549490986 SUCCESS**, build commit
  `b43cfd6aeb3fb1b22e792d7edb2bbdf42a355994`, artifact **10180866354**.
- Native canonical projection: ITN numeric-leading result preserved with
  punctuation; separate ITN=false result preserved with punctuation/uppercase.
- Bundled Windows FFmpeg codec tests: **10 passed**. Installation/upgrade:
  **9 scenarios passed**, including expected blocked-update exits.
- Final contracts: **34551115254 SUCCESS**, test commit
  `859d005b7e7aae412e625dd493e574ab66be3bcd`: Ubuntu **129**, Windows **123**,
  **80** Windows stress repetitions, Clippy on both platforms. Only tests/fake
  changed since the installer build; production inputs are identical.
- Public tag: `gigastt-desktop-v1.4.2-preview.1`, targeting the exact EXE build
  commit above. Asset: `Conversationaly-GigaSTT-1.4.2-x64-setup.exe`, **62,335,099 bytes**.
- SHA256: `e1d012d3a79a4084cd32e5f984f9b6277b61f0b92faf8e2c70276ad7a3b988c0`.
- Direct EXE and SHA256SUMS downloads were verified using anonymous `curl -q`;
  downloaded bytes match the installer tested on Windows.

[Public EXE](https://github.com/Genius229/conversationaly/releases/download/gigastt-desktop-v1.4.2-preview.1/Conversationaly-GigaSTT-1.4.2-x64-setup.exe)

This remains an unsigned manual-install preview; separate graceful-child-
shutdown, built-in updater lifecycle and physical/long-duration acceptance
boundaries in `implementation-status.md` remain open.
