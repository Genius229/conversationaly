use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::process::Stdio;
use std::thread::JoinHandle;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, watch};
use tokio::time::{timeout, Instant};

use super::device::{capture_args, enumeration_args, select_device, DeviceSelectionError};
use super::pcm::{PcmDecodeError, PcmDecoder};
use super::windows_job::KillOnCloseJob;

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureLimits {
    pub enumeration_timeout: Duration,
    pub first_pcm_timeout: Duration,
    pub quit_grace: Duration,
    pub reap_timeout: Duration,
}

impl Default for CaptureLimits {
    fn default() -> Self {
        Self {
            enumeration_timeout: Duration::from_secs(5),
            first_pcm_timeout: Duration::from_secs(10),
            quit_grace: Duration::from_secs(2),
            reap_timeout: Duration::from_secs(2),
        }
    }
}

impl CaptureLimits {
    fn validate(self) -> Result<Self, CaptureError> {
        if self.enumeration_timeout.is_zero()
            || self.first_pcm_timeout.is_zero()
            || self.quit_grace.is_zero()
            || self.reap_timeout.is_zero()
        {
            return Err(CaptureError::InvalidLimits);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CaptureError {
    #[error("DirectShow capture time limits must be non-zero")]
    InvalidLimits,
    #[error("could not spawn the owned DirectShow {stage} process ({kind:?})")]
    Spawn {
        stage: &'static str,
        kind: io::ErrorKind,
    },
    #[error("could not contain the owned DirectShow {stage} process")]
    Ownership { stage: &'static str },
    #[error("DirectShow device enumeration timed out")]
    EnumerationTimeout,
    #[error("DirectShow device enumeration pipe failed ({kind:?})")]
    EnumerationPipe { kind: io::ErrorKind },
    #[error("DirectShow device selection failed: {0}")]
    DeviceSelection(#[from] DeviceSelectionError),
    #[error("DirectShow capture produced no valid PCM before its deadline")]
    FirstPcmTimeout,
    #[error("DirectShow capture exited before producing valid PCM")]
    StartupExited,
    #[error("DirectShow capture produced a non-finite PCM sample")]
    NonFinitePcm,
    #[error("DirectShow capture ended with a truncated PCM sample")]
    TruncatedPcm,
    #[error("DirectShow PCM callback terminated unexpectedly")]
    CallbackPanicked,
    #[error("DirectShow capture process exited unexpectedly")]
    RuntimeExited,
    #[error("DirectShow {stream} pipe failed ({kind:?})")]
    RuntimePipe {
        stream: &'static str,
        kind: io::ErrorKind,
    },
    #[error("DirectShow capture startup was cancelled")]
    Cancelled,
    #[error("DirectShow capture cleanup failed")]
    Cleanup,
    #[error("DirectShow capture owner thread failed")]
    OwnerThread,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopReport {
    pub graceful: bool,
    pub force_killed: bool,
    pub samples_delivered: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Control {
    Running,
    Stop,
    Abort,
}

pub struct DirectShowCapture {
    control: watch::Sender<Control>,
    owner: Option<JoinHandle<Result<StopReport, CaptureError>>>,
    abort_on_drop: bool,
}

impl DirectShowCapture {
    pub async fn start<OnPcm, OnError>(
        ffmpeg_path: PathBuf,
        exact_name: String,
        on_pcm: OnPcm,
        on_error: OnError,
    ) -> Result<Self, CaptureError>
    where
        OnPcm: FnMut(&[f32]) + Send + 'static,
        OnError: FnMut(String) + Send + 'static,
    {
        Self::start_with_limits(
            ffmpeg_path,
            exact_name,
            on_pcm,
            on_error,
            CaptureLimits::default(),
        )
        .await
    }

    pub async fn start_with_limits<OnPcm, OnError>(
        ffmpeg_path: PathBuf,
        exact_name: String,
        on_pcm: OnPcm,
        on_error: OnError,
        limits: CaptureLimits,
    ) -> Result<Self, CaptureError>
    where
        OnPcm: FnMut(&[f32]) + Send + 'static,
        OnError: FnMut(String) + Send + 'static,
    {
        let limits = limits.validate()?;
        let (control, receiver) = watch::channel(Control::Running);
        let (startup, ready) = oneshot::channel();
        let owner = std::thread::Builder::new()
            .name("directshow-capture-owner".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| CaptureError::OwnerThread)?;
                runtime.block_on(run_owner(
                    ffmpeg_path,
                    exact_name,
                    on_pcm,
                    on_error,
                    limits,
                    receiver,
                    startup,
                ))
            })
            .map_err(|error| CaptureError::Spawn {
                stage: "owner",
                kind: error.kind(),
            })?;
        // This guard exists before the first await, so cancelling `start`
        // cannot orphan a worker that has already spawned a child.
        let mut pending = PendingStart::new(control, owner);
        match ready.await {
            Ok(Ok(())) => Ok(pending.into_capture()),
            Ok(Err(error)) => {
                pending.join_finished().await;
                Err(error)
            }
            Err(_) => {
                pending.abort();
                pending.join_finished().await;
                Err(CaptureError::OwnerThread)
            }
        }
    }

    pub fn request_stop(&self) {
        send_control(&self.control, Control::Stop);
    }

    pub async fn finish(mut self) -> Result<StopReport, CaptureError> {
        self.request_stop();
        self.abort_on_drop = false;
        let owner = self.owner.take().ok_or(CaptureError::OwnerThread)?;
        join_owner(owner).await
    }

    pub async fn stop(self) -> Result<StopReport, CaptureError> {
        self.request_stop();
        self.finish().await
    }
}

impl Drop for DirectShowCapture {
    fn drop(&mut self) {
        if self.abort_on_drop {
            send_control(&self.control, Control::Abort);
        }
    }
}

struct PendingStart {
    control: watch::Sender<Control>,
    owner: Option<JoinHandle<Result<StopReport, CaptureError>>>,
    abort_on_drop: bool,
}

impl PendingStart {
    fn new(
        control: watch::Sender<Control>,
        owner: JoinHandle<Result<StopReport, CaptureError>>,
    ) -> Self {
        Self {
            control,
            owner: Some(owner),
            abort_on_drop: true,
        }
    }

    fn abort(&self) {
        send_control(&self.control, Control::Abort);
    }

    fn into_capture(mut self) -> DirectShowCapture {
        self.abort_on_drop = false;
        DirectShowCapture {
            control: self.control.clone(),
            owner: self.owner.take(),
            abort_on_drop: true,
        }
    }

    async fn join_finished(&mut self) {
        self.abort_on_drop = false;
        if let Some(owner) = self.owner.take() {
            let _ = join_owner(owner).await;
        }
    }
}

impl Drop for PendingStart {
    fn drop(&mut self) {
        if self.abort_on_drop {
            self.abort();
        }
    }
}

fn send_control(sender: &watch::Sender<Control>, wanted: Control) {
    sender.send_if_modified(|current| {
        if *current == Control::Abort || *current == wanted {
            return false;
        }
        *current = wanted;
        true
    });
}

async fn join_owner(
    owner: JoinHandle<Result<StopReport, CaptureError>>,
) -> Result<StopReport, CaptureError> {
    tokio::task::spawn_blocking(move || owner.join())
        .await
        .map_err(|_| CaptureError::OwnerThread)?
        .map_err(|_| CaptureError::OwnerThread)?
}

struct OwnedChild {
    child: Child,
    _job: KillOnCloseJob,
}

fn command(path: &PathBuf) -> Command {
    let mut command = Command::new(path);
    command.kill_on_drop(true);
    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

async fn spawn_owned(
    mut command: Command,
    stage: &'static str,
    reap_timeout: Duration,
) -> Result<OwnedChild, CaptureError> {
    let mut child = command.spawn().map_err(|error| CaptureError::Spawn {
        stage,
        kind: error.kind(),
    })?;
    let job = match KillOnCloseJob::assign(&child) {
        Ok(job) => job,
        Err(_) => {
            kill_child_and_reap(&mut child, reap_timeout).await?;
            return Err(CaptureError::Ownership { stage });
        }
    };
    Ok(OwnedChild { child, _job: job })
}

async fn run_owner<OnPcm, OnError>(
    ffmpeg_path: PathBuf,
    exact_name: String,
    on_pcm: OnPcm,
    on_error: OnError,
    limits: CaptureLimits,
    mut control: watch::Receiver<Control>,
    startup: oneshot::Sender<Result<(), CaptureError>>,
) -> Result<StopReport, CaptureError>
where
    OnPcm: FnMut(&[f32]) + Send + 'static,
    OnError: FnMut(String) + Send + 'static,
{
    let token = match enumerate(&ffmpeg_path, &exact_name, limits, &mut control).await {
        Ok(token) => token,
        Err(error) => {
            let _ = startup.send(Err(error.clone()));
            return Err(error);
        }
    };
    run_capture(
        &ffmpeg_path,
        &token,
        on_pcm,
        on_error,
        limits,
        control,
        startup,
    )
    .await
}

async fn enumerate(
    ffmpeg_path: &PathBuf,
    exact_name: &str,
    limits: CaptureLimits,
    control: &mut watch::Receiver<Control>,
) -> Result<String, CaptureError> {
    let mut command = command(ffmpeg_path);
    command
        .args(enumeration_args())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut owned = spawn_owned(command, "enumeration", limits.reap_timeout).await?;
    let mut stderr = match owned.child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            force_kill_and_reap(&mut owned, limits.reap_timeout).await?;
            return Err(CaptureError::EnumerationPipe {
                kind: io::ErrorKind::BrokenPipe,
            });
        }
    };
    let deadline = Instant::now() + limits.enumeration_timeout;
    let mut retained = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        tokio::select! {
            read = stderr.read(&mut buffer) => match read {
                Ok(0) => break,
                Ok(count) => {
                    let remaining = super::device::ENUMERATION_BYTE_LIMIT
                        .saturating_add(1)
                        .saturating_sub(retained.len());
                    retained.extend_from_slice(&buffer[..count.min(remaining)]);
                }
                Err(error) => {
                    force_kill_and_reap(&mut owned, limits.reap_timeout).await?;
                    return Err(CaptureError::EnumerationPipe { kind: error.kind() });
                }
            },
            changed = control.changed() => {
                if changed.is_err() || *control.borrow() == Control::Abort {
                    force_kill_and_reap(&mut owned, limits.reap_timeout).await?;
                    return Err(CaptureError::Cancelled);
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                force_kill_and_reap(&mut owned, limits.reap_timeout).await?;
                return Err(CaptureError::EnumerationTimeout);
            }
        }
    }
    tokio::select! {
        status = owned.child.wait() => match status {
            Ok(_) => select_device(&retained, exact_name).map_err(CaptureError::from),
            Err(_) => Err(CaptureError::Cleanup),
        },
        _ = control.changed() => {
            force_kill_and_reap(&mut owned, limits.reap_timeout).await?;
            Err(CaptureError::Cancelled)
        },
        _ = tokio::time::sleep_until(deadline) => {
            force_kill_and_reap(&mut owned, limits.reap_timeout).await?;
            Err(CaptureError::EnumerationTimeout)
        }
    }
}

#[derive(Default)]
struct StderrStats {
    bytes: u64,
    lines: u64,
    error_chunks: u64,
    warning_chunks: u64,
    other_chunks: u64,
}

impl StderrStats {
    fn observe(&mut self, bytes: &[u8]) {
        self.bytes = self.bytes.saturating_add(bytes.len() as u64);
        self.lines = self
            .lines
            .saturating_add(bytes.iter().filter(|&&byte| byte == b'\n').count() as u64);
        if contains_ascii_case_insensitive(bytes, b"error") {
            self.error_chunks = self.error_chunks.saturating_add(1);
        } else if contains_ascii_case_insensitive(bytes, b"warning") {
            self.warning_chunks = self.warning_chunks.saturating_add(1);
        } else {
            self.other_chunks = self.other_chunks.saturating_add(1);
        }
    }
}

async fn run_capture<OnPcm, OnError>(
    ffmpeg_path: &PathBuf,
    token: &str,
    mut on_pcm: OnPcm,
    mut on_error: OnError,
    limits: CaptureLimits,
    mut control: watch::Receiver<Control>,
    startup_sender: oneshot::Sender<Result<(), CaptureError>>,
) -> Result<StopReport, CaptureError>
where
    OnPcm: FnMut(&[f32]) + Send + 'static,
    OnError: FnMut(String) + Send + 'static,
{
    let mut command = command(ffmpeg_path);
    command
        .args(capture_args(token))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut owned = match spawn_owned(command, "capture", limits.reap_timeout).await {
        Ok(child) => child,
        Err(error) => {
            let _ = startup_sender.send(Err(error.clone()));
            return Err(error);
        }
    };
    let mut stdin = owned.child.stdin.take();
    let Some(mut stdout) = owned.child.stdout.take() else {
        let error = CaptureError::RuntimePipe {
            stream: "stdout",
            kind: io::ErrorKind::BrokenPipe,
        };
        let result = force_kill_and_reap(&mut owned, limits.reap_timeout).await;
        let _ = startup_sender.send(Err(if result.is_ok() {
            error.clone()
        } else {
            CaptureError::Cleanup
        }));
        return result.and(Err(error));
    };
    let Some(mut stderr) = owned.child.stderr.take() else {
        let error = CaptureError::RuntimePipe {
            stream: "stderr",
            kind: io::ErrorKind::BrokenPipe,
        };
        let result = force_kill_and_reap(&mut owned, limits.reap_timeout).await;
        let _ = startup_sender.send(Err(if result.is_ok() {
            error.clone()
        } else {
            CaptureError::Cleanup
        }));
        return result.and(Err(error));
    };
    let mut startup = Some(startup_sender);
    let first_pcm_deadline = Instant::now() + limits.first_pcm_timeout;
    let mut stdout_open = true;
    let mut stdout_eof_deadline = None;
    let mut stderr_open = true;
    let mut child_status = None;
    let mut drain_deadline = None;
    let mut stop_deadline = None;
    let mut intentional = false;
    let mut force_killed = false;
    let mut samples_delivered = 0_u64;
    let mut decoder = PcmDecoder::new();
    let mut stderr_stats = StderrStats::default();
    let mut stdout_buffer = [0_u8; 4096];
    let mut stderr_buffer = [0_u8; 8192];
    let mut fatal = None;

    loop {
        if child_status.is_none() {
            match owned.child.try_wait() {
                Ok(Some(status)) => {
                    child_status = Some(status);
                    drain_deadline = Some(Instant::now() + limits.reap_timeout);
                }
                Ok(None) => {}
                Err(_) => {
                    fatal = Some(CaptureError::Cleanup);
                    break;
                }
            }
        }
        if child_status.is_some() && !stdout_open && !stderr_open {
            break;
        }
        let now = Instant::now();
        if startup.is_some() && now >= first_pcm_deadline {
            fatal = Some(CaptureError::FirstPcmTimeout);
            break;
        }
        if stop_deadline.is_some_and(|deadline| child_status.is_none() && now >= deadline) {
            match force_kill_and_reap(&mut owned, limits.reap_timeout).await {
                Ok(()) => {
                    force_killed = true;
                    child_status = owned.child.try_wait().ok().flatten();
                    drain_deadline = Some(Instant::now() + limits.reap_timeout);
                }
                Err(error) => {
                    fatal = Some(error);
                    break;
                }
            }
        }
        if drain_deadline.is_some_and(|deadline| now >= deadline && (stdout_open || stderr_open)) {
            fatal = Some(CaptureError::Cleanup);
            break;
        }
        if stdout_eof_deadline
            .is_some_and(|deadline| !intentional && child_status.is_none() && now >= deadline)
        {
            fatal = Some(CaptureError::RuntimePipe {
                stream: "stdout",
                kind: io::ErrorKind::UnexpectedEof,
            });
            break;
        }

        tokio::select! {
            read = stdout.read(&mut stdout_buffer), if stdout_open => match read {
                Ok(0) => {
                    stdout_open = false;
                    stdout_eof_deadline = Some(Instant::now() + Duration::from_millis(100));
                    let mut emitted = 0_u64;
                    let mut emit = |pcm: &[f32]| {
                        catch_unwind(AssertUnwindSafe(|| on_pcm(pcm))).map_err(|_| ())?;
                        emitted = emitted.saturating_add(pcm.len() as u64);
                        Ok(())
                    };
                    let result = decoder.finish(&mut emit);
                    note_delivery(emitted, &mut samples_delivered, &mut startup);
                    match result {
                        Ok(_) => {}
                        Err(error) => { fatal = Some(map_pcm_error(error)); break; }
                    }
                }
                Ok(count) => {
                    let mut emitted = 0_u64;
                    let mut emit = |pcm: &[f32]| {
                        catch_unwind(AssertUnwindSafe(|| on_pcm(pcm))).map_err(|_| ())?;
                        emitted = emitted.saturating_add(pcm.len() as u64);
                        Ok(())
                    };
                    let result = decoder.push(&stdout_buffer[..count], &mut emit);
                    note_delivery(emitted, &mut samples_delivered, &mut startup);
                    match result {
                        Ok(_) => {}
                        Err(error) => { fatal = Some(map_pcm_error(error)); break; }
                    }
                }
                Err(error) => {
                    fatal = Some(CaptureError::RuntimePipe { stream: "stdout", kind: error.kind() });
                    break;
                }
            },
            read = stderr.read(&mut stderr_buffer), if stderr_open => match read {
                Ok(0) => stderr_open = false,
                Ok(count) => stderr_stats.observe(&stderr_buffer[..count]),
                Err(error) => {
                    fatal = Some(CaptureError::RuntimePipe { stream: "stderr", kind: error.kind() });
                    break;
                }
            },
            changed = control.changed() => {
                let state = *control.borrow();
                if changed.is_err() || state == Control::Abort {
                    intentional = true;
                    match force_kill_and_reap(&mut owned, limits.reap_timeout).await {
                        Ok(()) => {
                            force_killed = true;
                            child_status = owned.child.try_wait().ok().flatten();
                            drain_deadline = Some(Instant::now() + limits.reap_timeout);
                        }
                        Err(error) => {
                            fatal = Some(error);
                            break;
                        }
                    }
                    if startup.is_some() { fatal = Some(CaptureError::Cancelled); }
                } else if state == Control::Stop && !intentional {
                    intentional = true;
                    stop_deadline = Some(Instant::now() + limits.quit_grace);
                    if let Some(mut input) = stdin.take() {
                        let write_limit = limits.quit_grace.min(Duration::from_millis(100));
                        let _ = timeout(write_limit, async {
                            input.write_all(b"q\n").await?;
                            input.flush().await?;
                            input.shutdown().await
                        }).await;
                    }
                }
            }
            _ = tokio::time::sleep(PROCESS_POLL_INTERVAL) => {}
        }
    }

    log::info!(
        "DirectShow FFmpeg stderr drained: bytes={}, line_boundaries={}, error_chunks={}, warning_chunks={}, other_chunks={}",
        stderr_stats.bytes,
        stderr_stats.lines,
        stderr_stats.error_chunks,
        stderr_stats.warning_chunks,
        stderr_stats.other_chunks,
    );
    if let Some(error) = fatal {
        if child_status.is_none()
            && force_kill_and_reap(&mut owned, limits.reap_timeout)
                .await
                .is_err()
        {
            return finish_error(
                CaptureError::Cleanup,
                intentional,
                &mut startup,
                &mut on_error,
            );
        }
        return finish_error(error, intentional, &mut startup, &mut on_error);
    }
    if let Some(ready) = startup.take() {
        let error = CaptureError::StartupExited;
        let _ = ready.send(Err(error.clone()));
        return Err(error);
    }
    if intentional {
        Ok(StopReport {
            graceful: !force_killed,
            force_killed,
            samples_delivered,
        })
    } else {
        let error = CaptureError::RuntimeExited;
        invoke_error(&mut on_error, error.to_string());
        Err(error)
    }
}

fn contains_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| {
        window
            .iter()
            .zip(needle)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
    })
}

fn note_delivery(
    delivered: u64,
    total: &mut u64,
    startup: &mut Option<oneshot::Sender<Result<(), CaptureError>>>,
) {
    *total = total.saturating_add(delivered);
    if delivered > 0 {
        if let Some(ready) = startup.take() {
            let _ = ready.send(Ok(()));
        }
    }
}

fn finish_error(
    error: CaptureError,
    intentional: bool,
    startup: &mut Option<oneshot::Sender<Result<(), CaptureError>>>,
    on_error: &mut impl FnMut(String),
) -> Result<StopReport, CaptureError> {
    if let Some(ready) = startup.take() {
        let _ = ready.send(Err(error.clone()));
    } else if !intentional {
        invoke_error(on_error, error.to_string());
    }
    Err(error)
}

fn invoke_error(callback: &mut impl FnMut(String), message: String) {
    let _ = catch_unwind(AssertUnwindSafe(|| callback(message)));
}

fn map_pcm_error(error: PcmDecodeError) -> CaptureError {
    match error {
        PcmDecodeError::NonFinite => CaptureError::NonFinitePcm,
        PcmDecodeError::Truncated => CaptureError::TruncatedPcm,
        PcmDecodeError::CallbackPanicked => CaptureError::CallbackPanicked,
    }
}

async fn force_kill_and_reap(
    owned: &mut OwnedChild,
    reap_timeout: Duration,
) -> Result<(), CaptureError> {
    kill_child_and_reap(&mut owned.child, reap_timeout).await
}

async fn kill_child_and_reap(
    child: &mut Child,
    reap_timeout: Duration,
) -> Result<(), CaptureError> {
    match child.try_wait() {
        Ok(Some(_)) => return Ok(()),
        Ok(None) => {}
        Err(_) => return Err(CaptureError::Cleanup),
    }
    if child.start_kill().is_err() && !matches!(child.try_wait(), Ok(Some(_))) {
        return Err(CaptureError::Cleanup);
    }
    match timeout(reap_timeout, child.wait()).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(_)) | Err(_) => Err(CaptureError::Cleanup),
    }
}
