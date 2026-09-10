# Windows desktop acceptance

Status: **not executed on physical Windows hardware**. This checklist complements
the automated native/contract/installer checks; a successful build does not
prove microphone behavior or recording durability.

## Build under test

Use the `conversationaly-gigastt-windows-unsigned-dev` artifact from a successful
`GigaSTT desktop Windows check` run on `feat/gigastt-post-transcription`.
Record the run URL and commit SHA. Do not use an artifact from a failed or
cancelled run. The build is CPU-only, with GigaSTT pinned to 2.18.0.

Latest verified build (includes the Settings catalog):
[run 34477334327](https://github.com/Genius229/conversationaly/actions/runs/34477334327),
commit `188d1c8`. [Download artifact 10152866908](https://github.com/Genius229/conversationaly/actions/runs/34477334327/artifacts/10152866908)
(GitHub login required; artifacts expire after 14 days).
Installer: `Conversationaly GigaSTT Dev_1.4.1_x64-setup.exe`.
SHA256: `27e5b83e7c26aaf3b46c8688803336a768e1c6270f7e8da2f3bbe6a215771fbe`.

The CI overlay uses `Conversationaly GigaSTT Dev` and identifier
`com.conversationaly.gigastt-dev`: it has a separate install/data profile from
regular Conversationaly. Models and settings must be installed/configured in
that development profile; production data is not an acceptance fixture.
Updating an existing GigaSTT Dev installation keeps the same application data
profile and model directory; this catalog-only update does not require another
model download.

## Short functional pass

- [ ] Install and launch on Windows x86-64 without Python, Docker or WSL.
- [ ] Confirm automatic final transcription is on and live transcript is off.
- [ ] With no GigaSTT models, verify recording still starts without a Parakeet
      download; Stop saves the audio and reports actionable missing models.
- [ ] Use the explicit model download action. Check progress, cancel, retry,
      successful verification and repair of a deliberately removed model file.
      In the catalog-update build, find it in **Settings → Transcription →
      On-device models → GigaSTT** (`After recording · Offline`). The original
      `119b77e` build exposes this control only on an existing meeting page.
- [ ] Disconnect networking after models are installed. Record 30–60 seconds of
      Russian speech. Confirm Stop saves first, then final transcription runs
      locally and reaches Ready with readable punctuation and timestamps.
- [ ] Confirm `transcripts.json`, `transcript.md` and the displayed final text
      agree after a successful import. Do not publish their contents as evidence.
- [ ] Re-transcribe an existing meeting. Cancel during preparation/transcription;
      confirm the original audio and prior transcript survive. Retry to Ready.
- [ ] Force a failed job (for example by closing the app while it runs), restart,
      and confirm interrupted status with retry instead of a permanent spinner.
- [ ] Confirm automatic summary waits for the final rows and, if enabled, speaker
      labelling completion. A failed required job must not summarize the draft
      automatically; explicit current/draft summary remains available.
- [ ] Complete an alternative-provider retranscription and confirm its new rows
      remain visible and the old GigaSTT badge is cleared.
- [ ] Close the application during model download and during transcription;
      confirm its owned GigaSTT process is reaped. Record shutdown mode accurately:
      current Windows termination is bounded force/reap, **not graceful**.

## Devices and long recording

- [ ] Select a USB microphone, restart the app and verify the selection persists.
- [ ] Verify null system input captures microphone only. Null microphone with an
      explicit system input must still capture the default microphone as well.
- [ ] Record for at least 60 minutes with live preview off. Sample app/sidecar
      working-set memory every five minutes; retain the numeric samples only.
- [ ] During recording and final transcription, navigate/scroll/change views;
      record any UI freezes and whether capture continues.
- [ ] Stop the long recording, verify expected duration and playability, complete
      final transcription and compare source-audio SHA256 before/after the job.

## Evidence to retain

Record OS version, CPU/RAM, microphone model, workflow run/commit, duration,
memory samples, stage timings and pass/fail per checkbox. Keep diagnostic audio
and transcripts local; never commit them. Sanitized failures should name the
stage and error code without speech content or credentials.

Release remains gated on this checklist and a supported, proven graceful
Windows sidecar shutdown mechanism. See `implementation-status.md` for current
automated evidence.
