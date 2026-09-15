# Standalone Windows capture diagnostic kit

Public tool release: **1.0.0**. This is not an app installer or a microphone fix.
Conversationaly 1.4.5 and `feat/gigastt-vad-toggle` remain unchanged.

[Download diagnostic ZIP without login](https://github.com/Genius229/conversationaly/releases/download/capture-diagnostics-v1.0.0/Conversationaly-Capture-Diagnostics-1.0.0.zip)

- File: `Conversationaly-Capture-Diagnostics-1.0.0.zip`
- Size: **14,549 bytes**
- SHA256: `87f7228104b1b3515cfa8876c6efdb753c4e07f2f32a3b8f3da698b280220947`
- Source branch: `feat/windows-capture-diagnostics`
- Tested source: `408021f1b99f22e15e00f6158c1dbc03eb345af1`
- [Windows CI](https://github.com/Genius229/conversationaly/actions/runs/34986878306): **PASS**
- Validated artifact: **10403454896**

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
devices, queries native input options and attempts up to two short captures of
one selected microphone: alternative moniker versus exact friendly name.
Capture stdout is drained as binary and discarded; only byte counts/timing are
kept. Error text/raw stderr is retained with a 128 KiB bound and explicit
truncation reporting. No recordings, transcripts or app settings are collected.
Reports contain device identifiers and local program paths; nothing is uploaded
automatically. The recipient manually decides to send the report.

Ambiguous names, malformed/invalid-UTF8/truncated/incomplete enumeration and
failed enumeration cleanup are rejected. Friendly names containing DirectShow's
`:` selector delimiter are not passed to FFmpeg; the safe moniker probe may
still run, and skipped probes carry an explicit reason. No camera/extra input
is implicitly selected. Input hardware formats are not forced by the output
`-ac 1 -ar 48000` conversion flags.

## Verification

Windows PowerShell **5.1.26100.33296** and PowerShell **7.6.5** each passed
**27 tests, 0 failures, 0 skips** in Windows CI. Checks include:

- Exact argument/identity handling, Unicode, quotes and backslashes.
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
microphone. The target-PC report is still needed to identify the actual
DirectShow failure. There is a non-atomic spawn-to-job-assignment interval;
no absolute containment claim is made for sudden parent death in that interval.
