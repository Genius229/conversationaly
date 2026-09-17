# Standalone Windows capture diagnostic kit

Public tool release: **1.1.0**. This is not an app installer or a microphone fix.
Conversationaly 1.4.5 and `feat/gigastt-vad-toggle` remain unchanged.

[Download diagnostic ZIP without login](https://github.com/Genius229/conversationaly/releases/download/capture-diagnostics-v1.1.0/Conversationaly-Capture-Diagnostics-1.1.0.zip)

- File: `Conversationaly-Capture-Diagnostics-1.1.0.zip`
- Size: **15,256 bytes**
- SHA256: `01d36a2edb3cce38ebf4a56566071c9137b7e08c5bdd62a21f22516451e5579c`
- Source branch: `feat/windows-capture-diagnostics`
- Tested source: `d29f25abc18a1da65e358406505667659360734e`
- [Windows CI](https://github.com/Genius229/conversationaly/actions/runs/35200736774): **PASS**
- Validated artifact: **10487982111**

The previous [1.0.0 release](https://github.com/Genius229/conversationaly/releases/tag/capture-diagnostics-v1.0.0)
is preserved; its archive was not replaced.

## Helper instructions

1. Extract the complete ZIP into a normal folder.
2. Quit Conversationaly through its tray menu. Close another app if it is
   currently using the microphone; do not change drivers or Windows defaults.
3. Double-click `START_DIAGNOSTICS.cmd`, without elevation.
4. Choose the exact microphone from the numbered list and wait.
5. Send the **newly generated report ZIP**, not just its local path and not the
   original tool ZIP. The console prints its path. If archiving fails, it prints
   the directory containing the preserved report.

The launcher uses built-in Windows PowerShell. No Python, Docker, WSL, new
FFmpeg download or application reinstallation is required. Talking is optional:
this checks stream startup, not recognition quality.

## Scope and privacy

The collector reads the installed Dev app's location and uses only its sibling
FFmpeg. It records Windows/build/time-zone and FFmpeg version/hash, enumerates
devices, queries native input options and attempts up to four short captures of
one selected microphone. The original alternative-moniker and exact-friendly-name
captures retain their default input format. Two additional captures use the same
safe alternative moniker with explicit input options before `-i`:

- `capture-pcm16-auto`: `-sample_size 16` only; input rate/channels remain automatic.
- `capture-pcm16-48000-mono`: `-sample_rate 48000 -sample_size 16 -channels 1`.

Here 16 means **bits per sample**, not 16 kHz. Each probe retains its independent
result and runs after a previous natural failure. A normal sequence has eight
processes: version, enumeration, two option queries and four captures.

Capture stdout is drained as binary and discarded; only byte counts/timing are
kept. Error text/raw stderr is retained with a 128 KiB bound and explicit
truncation reporting. No recordings, transcripts or app settings are collected.
Reports contain device identifiers and local program paths; nothing is uploaded
automatically. The recipient manually decides to send the report.

Ambiguous names, malformed/invalid-UTF8/truncated/incomplete enumeration and
failed enumeration cleanup are rejected. Friendly names containing DirectShow's
`:` selector delimiter are not passed to FFmpeg; the safe moniker probe may
still run, and skipped probes carry an explicit reason. No camera/extra input
is implicitly selected. All four captures retain the same output conversion
`-map 0:a:0 -ac 1 -ar 48000 -c:a pcm_f32le -f f32le pipe:1`; these output flags
do not select the input hardware format. Only the two new profiles add input
format constraints. They do not change saved application or Windows settings.

## Verification

Windows PowerShell **5.1.26100.33296** and PowerShell **7.6.5** each passed
**28 tests, 0 failures, 0 skips** in Windows CI. Checks include:

- Exact argument/identity handling, Unicode, quotes and backslashes.
- Unchanged baseline argv, exact input-profile flags before `-i`, unknown-profile
  rejection before launch, independent profile results after baseline failure,
  and safe-moniker profiles when a colon-containing friendly name is skipped.
- Full error capture, bounded stderr flooding and discarded binary stdout.
- Startup stall/early exit, exact native stdin `71-0A` quit bytes, graceful
  completion, forced owned-child/descendant cleanup with a Windows Job Object.
- Parent console input-encoding restoration on process start.
- Real hash-pinned released FFmpeg: finite generated PCM and paced infinite
  generated audio stopped by `q`, without physical microphone access.
- Report archive contents, output-folder fallback, partial report preservation,
  help/version and unknown argument rejection.

Initial Windows checks exposed the .NET Framework redirected-stdin behavior:
`Process.Start()` constructs a `StreamWriter(Console.InputEncoding)` and sets
`AutoFlush`, which can write an encoding preamble before any explicit input.
The final helper scopes BOM-less encoding around process startup under a lock,
restores the caller encoding, records restoration errors without losing child
ownership, and writes raw ASCII quit bytes. Failing tests were not waived.

The validated ZIP has exactly five entries: CMD launcher, PowerShell collector,
C# helper source, README and hash manifest. It contains no prebuilt EXE/DLL, models,
tests, captured audio or diagnostic data from a user. CRC, manifest file hashes,
source commit, public anonymous download and the adjacent SHA256SUMS were checked.

**Boundary:** CI validates the tool, not the problematic physical Windows 11
microphone. The new target-PC report is needed to establish whether either
explicit input format produces PCM where default negotiation failed. A completed
report is not proof of successful capture; byte counts, first-byte timing and
clean process shutdown must be checked. There is a non-atomic spawn-to-job-assignment interval;
no absolute containment claim is made for sudden parent death in that interval.
