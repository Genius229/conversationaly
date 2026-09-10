# Windows desktop acceptance

Status: **not executed on physical Windows hardware**. This checklist complements
the automated native/contract/installer checks; a successful build does not
prove microphone behavior or recording durability.

## Build under test

Use the `conversationaly-gigastt-windows-unsigned-dev` artifact from a successful
`GigaSTT desktop Windows check` or `GigaSTT installer repackage and acceptance`
run on `feat/gigastt-post-transcription`.
Record the run URL and commit SHA. Do not use an artifact from a failed or
cancelled run. The build is CPU-only, with GigaSTT pinned to 2.18.0.

Latest verified installer (OGG/Opus and M4A codec fallback, retaining Home/installer fixes):
[run 34536182311](https://github.com/Genius229/conversationaly/actions/runs/34536182311),
full native/desktop build and installer commit `6e3f417`.
[Download artifact 10176121731](https://github.com/Genius229/conversationaly/actions/runs/34536182311/artifacts/10176121731)
(GitHub login required; artifacts expire after 14 days).
Installer: `Conversationaly GigaSTT Dev_1.4.1_x64-setup.exe`.
SHA256: `c82b2731373d3c8ab0a46d553a3d2ed126b9b9377162522fe7d36ef5629ff0f6`.
All nine real installation/upgrade cases and ten codec tests passed. See
`import-codec-verification.md` and `home-installer-verification.md` for evidence
and remaining gates.

The CI overlay uses `Conversationaly GigaSTT Dev` and identifier
`com.conversationaly.gigastt-dev`: it has a separate install/data profile from
regular Conversationaly. Models and settings must be installed/configured in
that development profile; production data is not an acceptance fixture.
Updating an existing GigaSTT Dev installation keeps the same application data
profile and model directory; this update does not require another
model download.

Before updating an older build, choose **Quit** from its tray menu: closing the
window only hides it. New builds support an exact-path cooperative installer
quit request; an active recording refuses it. Older builds that ignore the
request receive one Retry/Cancel prompt instead of per-DLL Ignore dialogs.

## Short functional pass

- [ ] Install and launch on Windows x86-64 without Python, Docker or WSL.
- [ ] On a fresh setup, verify no model download starts automatically. Choose
      **Set up models later**, reach the app, restart and confirm no download
      starts. Existing completed profiles should not be reset just to update.
- [ ] Verify an explicit Download starts only that model; leaving an active
      user-started download is labelled **Continue downloads in background**.
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
- [ ] Use **Import audio → Import with GigaSTT** for an existing supported MP4.
      Confirm the source hash is unchanged, the archived audio is playable,
      final text appears and retry works after cancellation. No legacy speech
      model should be required. Cancel is available after staging opens the
      meeting page; application exit during staging must not leave a writer
      racing database shutdown.
- [ ] Import the previously failing Telegram OGG/Opus and M4A files. Verify
      validation completes, final text appears, and the original files remain
      unchanged. A retained failed meeting can be retried after updating.
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
