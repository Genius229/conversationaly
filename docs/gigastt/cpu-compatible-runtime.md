# GigaSTT Windows CPU compatibility runtime

Published preview: **CPU Compat 1**, 2026-09-17. This replaces only the GigaSTT
runtime used by Conversationaly GigaSTT Dev 1.4.5, not the application installer.

[Download without login](https://github.com/Genius229/conversationaly/releases/download/gigastt-runtime-v2.21.0-cpu-compat.1/GigaSTT-2.21.0-Windows-x64-CPU-Compat-1.zip)

- ZIP: `GigaSTT-2.21.0-Windows-x64-CPU-Compat-1.zip`
- Size: **12,006,709 bytes**
- SHA256: `c77c92506d850c877b686b9ce01f4b829d7057ac8ec18579c4952991d3fc0b3c`
- Tested build source: `25dcf6f3d66627d9645e0e90cf41d0ff4f47a8e4`
- Branch: `feat/gigastt-cpu-compat`, retained separately without merging.
- [Same-SHA native Windows + IVB gate](https://github.com/Genius229/conversationaly/actions/runs/35222436841): **success**.
- Runtime artifact: `10498160305`; native evidence: `10498150276`;
  CPU instruction evidence: `10498920326`.

## Why a separate runtime

The old packaged GigaSTT uses ort 2.0.0-rc.13's statically linked Pyke ONNX
Runtime, which explicitly requires AVX2. Intel Core i3-3110M supports AVX but
not AVX2. The reported `0xC000001D` is an illegal-instruction exception.
The old runtime reproduced an illegal BMI2 `shlx` under the IVB instruction
checker before producing a transcript.

The compatible profile retains GigaSTT **2.21.0**, upstream source
`5289e8a6b3a67217711e6b5cba6c2f19dec3c5eb`, and all eight pinned model assets.
It selects upstream `gigastt-core/ort-load-dynamic`, disables default native
diarization, explicitly uses Rust `target-cpu=x86-64`, and stages official
Microsoft CPU ONNX Runtime **1.28.2** next to the executable.

The dynamic ort-sys path disables native linking before the Pyke download/link
logic; changing Rust flags alone would not have fixed an embedded native library.
Legacy builds remain the default unless `-CpuCompatible` is supplied.

Models remain in `models/gigastt/2.18.0`. The model-layout version is not the
runtime version. VAD settings, microphone capture, database, recordings,
formatting contract and other application engines are unchanged.

## Acceptance actually performed

- Portable profile/staging tests: **12 passed**.
- SDE assertion tests: **5 passed**.
- Native Windows: version/help, DLL closure, jobs API, pinned four-second Russian
  fixture, punctuation, separate casing check and Tauri resource layout passed.
- Intel SDE **10.13.1**, **IVB**, native Windows Server 2022:
  - explicit AVX2 negative control rejected;
  - exact original app 1.4.5 runtime rejected;
  - candidate `--help` and `--version` succeeded;
  - candidate RNNT + VAD + punctuation + ITN inference succeeded: **6 words,
    4.0 seconds** of input, **97,738 ms under instrumentation**.
  - the instruction checker included the EXE and **every app-local DLL**.
- Final EXE SHA256:
  `c102d1ed5ceb066384ce37a079d2413b95f41d9400c42ffa282b032362bf7945`.
- ORT DLL SHA256:
  `1becbd71adbf49609d33195e29c9214969db3c3de69c81425bebe5a4b69aef97`.
- All ZIP member hashes/allowlists/CRC, build/evidence commit identity, negative
  control signatures and positive transcript fields were independently checked.
- Anonymous public download matched the validated ZIP byte-for-byte.

**Not claimed:** acceptance on the physical i3-3110M, real-CPU throughput from
the instrumented duration, long-recording acceptance, or pre-AVX2 compatibility
of the separate built-in Whisper/GigaAM/summary engines. Native smoke shutdown
remains the existing `forced-reaped` path; graceful Windows shutdown is not
fixed or reclassified by this runtime work.

## Instruction-test environment findings

The first all-DLL IVB run on Server 2025 reached a VCRUNTIME140.dll AVX2
`memcmp` variant. Exact PE analysis showed a Windows Function Override BDD:
AMD vendor plus AVX2 selects RVA `0x1e490`; Intel/default uses scalar
`0x1da00`. The host loader can choose a function using physical-host features
before SDE changes CPUID. No instruction-check exclusion was added.

The strict test was instead run on Server 2022. All DLL bytes stayed identical;
the rebuilt EXE differed only in 24 bytes of COFF/debug timestamps and PDB GUID,
not executable code/data/relocation sections. The complete all-DLL check then
passed. The target-machine boundary remains explicit in `VALIDATION.json`.

Host Schannel/bsdtar SDE preparation stalled; the CI bootstrap now uses bounded
Git Bash curl/GNU tar with the same pinned Intel archive hash. Intel SDE is not
included in the downloadable runtime.

## Installation test and rollback

1. Extract the ZIP separately; in PowerShell inside its `gigastt` folder run
   `./gigastt.exe --version` and `./gigastt.exe --help`.
2. Fully quit Conversationaly using tray → Quit.
3. In `%LOCALAPPDATA%\Conversationaly GigaSTT Dev`, rename the original
   `gigastt` folder to `gigastt-backup-1.4.5`.
4. Copy the **entire** `gigastt` folder from the ZIP into that installation
   directory. The executable alone is insufficient.
5. Restart the app and retry the previously imported recording.

No model download or database reset is needed. To roll back, quit the app,
move the candidate folder out and restore the backup's original name. The ZIP
contains a Russian README, redistribution notices, runtime inventory and
validation receipt, but no models, test tools, audio or user diagnostics.
