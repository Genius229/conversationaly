//! Managed lifecycle for the bundled GigaSTT 2.18.0 HTTP sidecar.
//!
//! This module deliberately has no Tauri dependency. The native adapter
//! resolves resource/app-data paths and supplies them through
//! [`GigasttSidecarConfig`]. A bind preflight rejects any pre-existing listener
//! before this manager spawns and retains its child. GigaSTT exposes no
//! per-process readiness nonce, so the small release-before-spawn window remains
//! a documented same-user local-trust boundary.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;
use tokio::time::{sleep, sleep_until, timeout, Instant};
use tokio_util::sync::CancellationToken;

use super::{GigasttClient, ReadinessStatus};

/// Source version whose CLI contract is encoded by [`server_arguments`].
pub const PINNED_GIGASTT_VERSION: &str = "2.18.0";

const LOOPBACK_HOST: &str = "127.0.0.1";
const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(250);
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_GRACEFUL_TIMEOUT: Duration = Duration::from_secs(3);
const DEFAULT_FORCE_KILL_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_DIAGNOSTIC_CAPACITY: usize = 128;
const DEFAULT_OUTPUT_CHUNK_BYTES: usize = 4096;
const SHUTDOWN_DRAIN_SECS: &str = "1";

/// Construction parameters after a Tauri adapter has resolved native paths.
#[derive(Debug, Clone)]
pub struct GigasttSidecarConfig {
    binary_path: PathBuf,
    model_dir: PathBuf,
    port: Option<u16>,
    startup_timeout: Duration,
    poll_interval: Duration,
    request_timeout: Duration,
    graceful_shutdown_timeout: Duration,
    force_kill_timeout: Duration,
    diagnostic_capacity: usize,
    output_chunk_bytes: usize,
}

impl GigasttSidecarConfig {
    /// `None` selects an ephemeral loopback port. An explicit non-zero port is
    /// useful for deterministic native smoke tests.
    pub fn new(
        binary_path: impl Into<PathBuf>,
        model_dir: impl Into<PathBuf>,
        port: Option<u16>,
    ) -> Self {
        Self {
            binary_path: binary_path.into(),
            model_dir: model_dir.into(),
            port,
            startup_timeout: DEFAULT_STARTUP_TIMEOUT,
            poll_interval: DEFAULT_POLL_INTERVAL,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            graceful_shutdown_timeout: DEFAULT_GRACEFUL_TIMEOUT,
            force_kill_timeout: DEFAULT_FORCE_KILL_TIMEOUT,
            diagnostic_capacity: DEFAULT_DIAGNOSTIC_CAPACITY,
            output_chunk_bytes: DEFAULT_OUTPUT_CHUNK_BYTES,
        }
    }

    pub fn with_startup_timeout(mut self, value: Duration) -> Self {
        self.startup_timeout = value;
        self
    }

    pub fn with_poll_interval(mut self, value: Duration) -> Self {
        self.poll_interval = value;
        self
    }

    pub fn with_request_timeout(mut self, value: Duration) -> Self {
        self.request_timeout = value;
        self
    }

    pub fn with_shutdown_timeouts(mut self, graceful: Duration, force: Duration) -> Self {
        self.graceful_shutdown_timeout = graceful;
        self.force_kill_timeout = force;
        self
    }

    /// Bound the retained record count and temporary child-output buffer.
    /// Child bytes themselves are never retained.
    pub fn with_diagnostic_limits(mut self, capacity: usize, output_chunk_bytes: usize) -> Self {
        self.diagnostic_capacity = capacity;
        self.output_chunk_bytes = output_chunk_bytes;
        self
    }

    fn validate(&self) -> Result<(), SidecarError> {
        if self.port == Some(0) {
            return Err(SidecarError::InvalidConfig(
                "an explicit sidecar port must be non-zero".into(),
            ));
        }
        if self.binary_path.as_os_str().is_empty() || self.model_dir.as_os_str().is_empty() {
            return Err(SidecarError::InvalidConfig(
                "binary and model paths must not be empty".into(),
            ));
        }
        if self.startup_timeout.is_zero()
            || self.poll_interval.is_zero()
            || self.request_timeout.is_zero()
            || self.force_kill_timeout.is_zero()
            || self.diagnostic_capacity == 0
            || self.output_chunk_bytes == 0
        {
            return Err(SidecarError::InvalidConfig(
                "timeouts and diagnostic limits must be non-zero".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidecarStatus {
    NotInstalled,
    Starting,
    Ready,
    Busy,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidecarFailureKind {
    BinaryNotFound,
    ModelDirectory,
    Spawn,
    PortUnavailable,
    StartupTimeout,
    UnexpectedExit,
    Cancelled,
    Cleanup,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarFailure {
    pub kind: SidecarFailureKind,
    pub message: String,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarSnapshot {
    pub status: SidecarStatus,
    pub failure: Option<SidecarFailure>,
    pub port: Option<u16>,
    pub restart_count: u8,
}

impl Default for SidecarSnapshot {
    fn default() -> Self {
        Self {
            status: SidecarStatus::NotInstalled,
            failure: None,
            port: None,
            restart_count: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticKind {
    ProcessSpawned,
    ChildOutput,
    ProcessExited,
    Restarting,
    ReadinessTimeout,
    GracefulShutdown,
    ForceKill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticStream {
    Stdout,
    Stderr,
}

/// Privacy-preserving process diagnostic. Child output is drained, but only
/// byte counts are retained; no stdout/stderr bytes survive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarDiagnostic {
    pub sequence: u64,
    pub kind: DiagnosticKind,
    pub stream: Option<DiagnosticStream>,
    pub byte_count: usize,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SidecarError {
    #[error("invalid GigaSTT sidecar configuration: {0}")]
    InvalidConfig(String),
    #[error("GigaSTT {PINNED_GIGASTT_VERSION} binary was not found at {path}")]
    BinaryNotFound { path: PathBuf },
    #[error("could not prepare the GigaSTT model directory {path}: {message}")]
    ModelDirectory { path: PathBuf, message: String },
    #[error("could not start GigaSTT sidecar: {message}")]
    SpawnFailed { message: String },
    #[error("loopback port {port} is already in use; refusing to adopt its server")]
    PortUnavailable { port: u16 },
    #[error("GigaSTT sidecar did not become ready within {timeout_ms} ms")]
    StartupTimeout { timeout_ms: u64 },
    #[error("GigaSTT sidecar exited unexpectedly (code {code:?}, after_restart={after_restart})")]
    ProcessExited {
        code: Option<i32>,
        after_restart: bool,
    },
    #[error("GigaSTT sidecar startup was cancelled")]
    Cancelled,
    #[error("GigaSTT sidecar manager is permanently closed")]
    Closed,
    #[error("GigaSTT sidecar cannot change busy state while {status:?}")]
    InvalidState { status: SidecarStatus },
    #[error("GigaSTT sidecar supervisor did not stop within its bounded timeout")]
    ShutdownTimeout,
    #[error("GigaSTT sidecar cleanup failed: {message}")]
    CleanupFailed { message: String },
}

struct RuntimeHandle {
    cancel: CancellationToken,
    join: JoinHandle<Result<(), SidecarError>>,
}

struct Inner {
    config: GigasttSidecarConfig,
    start_gate: Mutex<()>,
    runtime: StdMutex<Option<RuntimeHandle>>,
    stopping: AtomicBool,
    closed: AtomicBool,
    snapshot: watch::Sender<SidecarSnapshot>,
    diagnostics: Arc<Diagnostics>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        let runtime = self.runtime.get_mut().ok().and_then(Option::take);
        if let Some(runtime) = runtime {
            runtime.cancel.cancel();
            // Aborting drops the supervisor-owned Child. `kill_on_drop(true)`
            // is the last-resort backup when explicit async shutdown was missed.
            runtime.join.abort();
        }
    }
}

#[derive(Clone)]
pub struct GigasttSidecar {
    inner: Arc<Inner>,
}

impl GigasttSidecar {
    pub fn new(config: GigasttSidecarConfig) -> Result<Self, SidecarError> {
        config.validate()?;
        let (snapshot, _) = watch::channel(SidecarSnapshot::default());
        let diagnostics = Arc::new(Diagnostics::new(config.diagnostic_capacity));
        Ok(Self {
            inner: Arc::new(Inner {
                config,
                start_gate: Mutex::new(()),
                runtime: StdMutex::new(None),
                stopping: AtomicBool::new(false),
                closed: AtomicBool::new(false),
                snapshot,
                diagnostics,
            }),
        })
    }

    pub async fn status(&self) -> SidecarStatus {
        self.inner.snapshot.borrow().status
    }

    pub async fn snapshot(&self) -> SidecarSnapshot {
        self.inner.snapshot.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<SidecarSnapshot> {
        self.inner.snapshot.subscribe()
    }

    pub async fn diagnostics(&self) -> Vec<SidecarDiagnostic> {
        self.inner.diagnostics.snapshot()
    }

    /// Start the owned process if needed and wait for bounded readiness.
    /// Concurrent callers share one supervisor and child.
    pub async fn ensure_ready(&self) -> Result<GigasttClient, SidecarError> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(SidecarError::Closed);
        }
        if self.inner.stopping.load(Ordering::Acquire) {
            return Err(SidecarError::Cancelled);
        }

        let (mut status_rx, cancel) = {
            let _gate = self.inner.start_gate.lock().await;
            if self.inner.closed.load(Ordering::Acquire) {
                return Err(SidecarError::Closed);
            }
            if self.inner.stopping.load(Ordering::Acquire) {
                return Err(SidecarError::Cancelled);
            }

            let mut runtime = self.inner.runtime.lock().expect("sidecar runtime poisoned");
            if runtime.is_none() {
                let cancel = CancellationToken::new();
                let supervisor = Supervisor {
                    config: self.inner.config.clone(),
                    snapshot: self.inner.snapshot.clone(),
                    diagnostics: self.inner.diagnostics.clone(),
                };
                let task_cancel = cancel.clone();
                let join = tokio::spawn(async move { supervisor.run(task_cancel).await });
                *runtime = Some(RuntimeHandle { cancel, join });
            }
            let cancel = runtime.as_ref().expect("runtime inserted").cancel.clone();
            (self.inner.snapshot.subscribe(), cancel)
        };

        loop {
            let snapshot = status_rx.borrow().clone();
            match snapshot.status {
                SidecarStatus::Ready | SidecarStatus::Busy => {
                    let port = snapshot.port.ok_or_else(|| {
                        SidecarError::InvalidConfig("ready sidecar has no selected port".into())
                    })?;
                    // Startup probes use the short configured timeout, but a
                    // returned client also uploads long recordings and fetches
                    // results; retain the client's normal request budget.
                    return GigasttClient::new(format!("http://{LOOPBACK_HOST}:{port}"))
                        .map_err(|error| SidecarError::InvalidConfig(error.to_string()));
                }
                SidecarStatus::Failed => {
                    return Err(error_from_snapshot(&snapshot, &self.inner.config));
                }
                SidecarStatus::NotInstalled if snapshot.failure.is_some() => {
                    return Err(error_from_snapshot(&snapshot, &self.inner.config));
                }
                SidecarStatus::NotInstalled | SidecarStatus::Starting => {}
            }

            tokio::select! {
                _ = cancel.cancelled() => return Err(SidecarError::Cancelled),
                changed = status_rx.changed() => {
                    if changed.is_err() {
                        return Err(SidecarError::Cancelled);
                    }
                }
            }
        }
    }

    /// Update the public Ready/Busy state around a job. Lifecycle failure wins
    /// over service-level transitions.
    pub async fn set_busy(&self, busy: bool) -> Result<(), SidecarError> {
        let mut result = Ok(());
        self.inner.snapshot.send_if_modified(|current| {
            match (current.status, busy) {
                (SidecarStatus::Ready, true) => current.status = SidecarStatus::Busy,
                (SidecarStatus::Busy, false) => current.status = SidecarStatus::Ready,
                (SidecarStatus::Ready, false) | (SidecarStatus::Busy, true) => return false,
                (status, _) => {
                    result = Err(SidecarError::InvalidState { status });
                    return false;
                }
            }
            true
        });
        result
    }

    /// Stop, terminate within a bound, and wait for child reaping. GigaSTT
    /// 2.18.0 has no HTTP/stdin shutdown endpoint. Unix uses its SIGTERM path;
    /// Windows truthfully uses bounded force/reap rather than mismatched
    /// CTRL_BREAK. A later explicit `ensure_ready` starts a fresh lifecycle.
    pub async fn shutdown(&self) -> Result<(), SidecarError> {
        self.shutdown_inner().await
    }

    /// Permanently close the manager for application exit. Unlike `shutdown`,
    /// no later holder of a cloned manager can start a new child.
    pub async fn close(&self) -> Result<(), SidecarError> {
        self.inner.closed.store(true, Ordering::Release);
        self.shutdown_inner().await
    }

    async fn shutdown_inner(&self) -> Result<(), SidecarError> {
        self.inner.stopping.store(true, Ordering::Release);
        // Keep the lifecycle gate until the old child is reaped. A concurrent
        // shutdown sets `stopping` before it queues here, and reasserts it after
        // acquiring the gate so another caller cannot clear its cancellation.
        let _gate = self.inner.start_gate.lock().await;
        self.inner.stopping.store(true, Ordering::Release);
        let runtime = self
            .inner
            .runtime
            .lock()
            .expect("sidecar runtime poisoned")
            .take();

        let result = if let Some(mut runtime) = runtime {
            runtime.cancel.cancel();
            let budget = self
                .inner
                .config
                .graceful_shutdown_timeout
                .saturating_add(self.inner.config.force_kill_timeout)
                .saturating_add(Duration::from_secs(1));
            match timeout(budget, &mut runtime.join).await {
                Ok(Ok(result)) => result,
                Ok(Err(error)) => Err(SidecarError::CleanupFailed {
                    message: format!("supervisor task failed: {error}"),
                }),
                Err(_) => {
                    runtime.join.abort();
                    let _ = runtime.join.await;
                    Err(SidecarError::ShutdownTimeout)
                }
            }
        } else {
            Ok(())
        };

        let current = self.inner.snapshot.borrow().clone();
        let prior_cleanup_failure = current
            .failure
            .as_ref()
            .is_some_and(|failure| failure.kind == SidecarFailureKind::Cleanup);
        if result.is_ok() && !prior_cleanup_failure {
            self.inner.snapshot.send_replace(SidecarSnapshot::default());
        } else if result.is_err() {
            // Without proof of reaping, never permit another child to overlap
            // the possibly-live process on this manager.
            self.inner.closed.store(true, Ordering::Release);
            self.inner.snapshot.send_replace(SidecarSnapshot {
                status: SidecarStatus::Failed,
                failure: Some(SidecarFailure {
                    kind: SidecarFailureKind::Cleanup,
                    message: "sidecar cleanup did not complete".into(),
                    exit_code: None,
                }),
                ..current
            });
        }
        self.inner.stopping.store(false, Ordering::Release);
        result
    }
}

fn error_from_snapshot(snapshot: &SidecarSnapshot, config: &GigasttSidecarConfig) -> SidecarError {
    let Some(failure) = snapshot.failure.as_ref() else {
        return SidecarError::Cancelled;
    };
    match failure.kind {
        SidecarFailureKind::BinaryNotFound => SidecarError::BinaryNotFound {
            path: config.binary_path.clone(),
        },
        SidecarFailureKind::ModelDirectory => SidecarError::ModelDirectory {
            path: config.model_dir.clone(),
            message: failure.message.clone(),
        },
        SidecarFailureKind::Spawn => SidecarError::SpawnFailed {
            message: failure.message.clone(),
        },
        SidecarFailureKind::PortUnavailable => SidecarError::PortUnavailable {
            port: snapshot.port.unwrap_or_default(),
        },
        SidecarFailureKind::StartupTimeout => SidecarError::StartupTimeout {
            timeout_ms: duration_millis(config.startup_timeout),
        },
        SidecarFailureKind::UnexpectedExit => SidecarError::ProcessExited {
            code: failure.exit_code,
            after_restart: snapshot.restart_count > 0,
        },
        SidecarFailureKind::Cancelled => SidecarError::Cancelled,
        SidecarFailureKind::Cleanup => SidecarError::CleanupFailed {
            message: failure.message.clone(),
        },
    }
}

struct Supervisor {
    config: GigasttSidecarConfig,
    snapshot: watch::Sender<SidecarSnapshot>,
    diagnostics: Arc<Diagnostics>,
}

impl Supervisor {
    async fn run(self, cancel: CancellationToken) -> Result<(), SidecarError> {
        if !self.config.binary_path.is_file() {
            self.publish_failure(
                SidecarStatus::NotInstalled,
                SidecarFailureKind::BinaryNotFound,
                "bundled executable is missing",
                None,
                None,
                0,
            );
            return Ok(());
        }
        if let Err(error) = tokio::fs::create_dir_all(&self.config.model_dir).await {
            self.publish_failure(
                SidecarStatus::Failed,
                SidecarFailureKind::ModelDirectory,
                error.to_string(),
                None,
                self.config.port,
                0,
            );
            return Ok(());
        }
        // Side assets also stay below the versioned app-controlled root.
        for directory in [
            self.config.model_dir.join("punct"),
            self.config.model_dir.join("vad"),
        ] {
            if let Err(error) = tokio::fs::create_dir_all(&directory).await {
                self.publish_failure(
                    SidecarStatus::Failed,
                    SidecarFailureKind::ModelDirectory,
                    error.to_string(),
                    None,
                    self.config.port,
                    0,
                );
                return Ok(());
            }
        }

        let port = match self.config.port {
            Some(port) => port,
            None => match select_ephemeral_loopback_port() {
                Ok(port) => port,
                Err(error) => {
                    self.publish_failure(
                        SidecarStatus::Failed,
                        SidecarFailureKind::Spawn,
                        error.to_string(),
                        None,
                        None,
                        0,
                    );
                    return Ok(());
                }
            },
        };

        let client = match GigasttClient::with_timeout(
            format!("http://{LOOPBACK_HOST}:{port}"),
            self.config.request_timeout,
        ) {
            Ok(client) => client,
            Err(error) => {
                self.publish_failure(
                    SidecarStatus::Failed,
                    SidecarFailureKind::Spawn,
                    error.to_string(),
                    None,
                    Some(port),
                    0,
                );
                return Ok(());
            }
        };

        let mut restart_count = 0_u8;
        loop {
            if cancel.is_cancelled() {
                self.publish_stopped(Some(port), restart_count);
                return Ok(());
            }
            if let Err(error) = ensure_loopback_port_available(port) {
                self.publish_failure(
                    SidecarStatus::Failed,
                    SidecarFailureKind::PortUnavailable,
                    error.to_string(),
                    None,
                    Some(port),
                    restart_count,
                );
                return Ok(());
            }
            self.publish(SidecarStatus::Starting, None, Some(port), restart_count);
            let mut process = match self.spawn_process(port).await {
                Ok(process) => process,
                Err(error) => {
                    self.publish_failure(
                        SidecarStatus::Failed,
                        SidecarFailureKind::Spawn,
                        error.to_string(),
                        None,
                        Some(port),
                        restart_count,
                    );
                    return Ok(());
                }
            };

            let deadline = Instant::now() + self.config.startup_timeout;
            match self
                .wait_until_ready(&mut process.child, &client, &cancel, deadline)
                .await
            {
                StartupOutcome::Ready => {
                    self.publish(SidecarStatus::Ready, None, Some(port), restart_count);
                    match self.monitor(&mut process.child, &cancel).await {
                        MonitorOutcome::Cancelled => {
                            if let Err(error) = self.stop_and_reap(&mut process).await {
                                self.publish_cleanup_failure(
                                    error.to_string(),
                                    Some(port),
                                    restart_count,
                                );
                                return Err(error);
                            }
                            self.publish_stopped(Some(port), restart_count);
                            return Ok(());
                        }
                        MonitorOutcome::Exited(status) => {
                            process.finish_output().await;
                            self.diagnostics.push(
                                DiagnosticKind::ProcessExited,
                                None,
                                0,
                                status.code(),
                            );
                            if restart_count == 0 {
                                restart_count = 1;
                                self.diagnostics.push(
                                    DiagnosticKind::Restarting,
                                    None,
                                    0,
                                    status.code(),
                                );
                                continue;
                            }
                            self.publish_failure(
                                SidecarStatus::Failed,
                                SidecarFailureKind::UnexpectedExit,
                                "owned process exited after its one automatic restart",
                                status.code(),
                                Some(port),
                                restart_count,
                            );
                            return Ok(());
                        }
                    }
                }
                StartupOutcome::Cancelled => {
                    if let Err(error) = self.stop_and_reap(&mut process).await {
                        self.publish_cleanup_failure(error.to_string(), Some(port), restart_count);
                        return Err(error);
                    }
                    self.publish_stopped(Some(port), restart_count);
                    return Ok(());
                }
                StartupOutcome::TimedOut => {
                    self.diagnostics
                        .push(DiagnosticKind::ReadinessTimeout, None, 0, None);
                    if let Err(error) = self.stop_and_reap(&mut process).await {
                        self.publish_cleanup_failure(error.to_string(), Some(port), restart_count);
                        return Err(error);
                    }
                    self.publish_failure(
                        SidecarStatus::Failed,
                        SidecarFailureKind::StartupTimeout,
                        "owned process did not become ready before the startup deadline",
                        None,
                        Some(port),
                        restart_count,
                    );
                    return Ok(());
                }
                StartupOutcome::Exited(status) => {
                    process.finish_output().await;
                    self.diagnostics
                        .push(DiagnosticKind::ProcessExited, None, 0, status.code());
                    if restart_count == 0 {
                        restart_count = 1;
                        self.diagnostics
                            .push(DiagnosticKind::Restarting, None, 0, status.code());
                        continue;
                    }
                    self.publish_failure(
                        SidecarStatus::Failed,
                        SidecarFailureKind::UnexpectedExit,
                        "owned process exited before readiness after its one automatic restart",
                        status.code(),
                        Some(port),
                        restart_count,
                    );
                    return Ok(());
                }
            }
        }
    }

    async fn spawn_process(&self, port: u16) -> Result<OwnedProcess, std::io::Error> {
        let mut command = Command::new(&self.config.binary_path);
        command
            .args(server_arguments(&self.config.model_dir, port))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // clap accepts many GIGASTT_* environment overrides. The desktop app
        // owns this profile, so inherited shell/service variables must not add
        // hotwords, extra pools, public binds, or alternate model behavior.
        for (name, _) in std::env::vars_os() {
            if is_gigastt_environment_name(&name) {
                command.env_remove(name);
            }
        }

        #[cfg(target_os = "windows")]
        {
            // Hide the console only. Do not set CREATE_NEW_PROCESS_GROUP:
            // GigaSTT listens for CTRL_C, while Windows can target a group only
            // with CTRL_BREAK, so that would not be graceful.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = command.spawn()?;
        self.diagnostics
            .push(DiagnosticKind::ProcessSpawned, None, 0, None);
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let diagnostics = self.diagnostics.clone();
        let chunk_bytes = self.config.output_chunk_bytes;
        let stdout_task = stdout.map(|reader| {
            tokio::spawn(drain_output(
                reader,
                DiagnosticStream::Stdout,
                diagnostics.clone(),
                chunk_bytes,
            ))
        });
        let stderr_task = stderr.map(|reader| {
            tokio::spawn(drain_output(
                reader,
                DiagnosticStream::Stderr,
                diagnostics,
                chunk_bytes,
            ))
        });
        Ok(OwnedProcess {
            child,
            stdout_task,
            stderr_task,
        })
    }

    async fn wait_until_ready(
        &self,
        child: &mut Child,
        client: &GigasttClient,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> StartupOutcome {
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                return StartupOutcome::Exited(status);
            }
            if Instant::now() >= deadline {
                return StartupOutcome::TimedOut;
            }
            tokio::select! {
                _ = cancel.cancelled() => return StartupOutcome::Cancelled,
                _ = sleep_until(deadline) => return StartupOutcome::TimedOut,
                response = client.ready() => {
                    if matches!(response, Ok(value) if value.status == ReadinessStatus::Ready) {
                        if let Ok(Some(status)) = child.try_wait() {
                            return StartupOutcome::Exited(status);
                        }
                        return StartupOutcome::Ready;
                    }
                }
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            tokio::select! {
                _ = cancel.cancelled() => return StartupOutcome::Cancelled,
                _ = sleep(self.config.poll_interval.min(remaining)) => {}
            }
        }
    }

    async fn monitor(&self, child: &mut Child, cancel: &CancellationToken) -> MonitorOutcome {
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                return MonitorOutcome::Exited(status);
            }
            tokio::select! {
                _ = cancel.cancelled() => return MonitorOutcome::Cancelled,
                _ = sleep(self.config.poll_interval) => {}
            }
        }
    }

    async fn stop_and_reap(&self, process: &mut OwnedProcess) -> Result<(), SidecarError> {
        match process.child.try_wait() {
            Ok(Some(_)) => {
                process.finish_output().await;
                return Ok(());
            }
            Ok(None) => {}
            Err(error) => {
                return Err(SidecarError::CleanupFailed {
                    message: format!("could not inspect child state: {error}"),
                });
            }
        }

        if request_graceful_termination(&mut process.child) {
            self.diagnostics
                .push(DiagnosticKind::GracefulShutdown, None, 0, None);
            match timeout(self.config.graceful_shutdown_timeout, process.child.wait()).await {
                Ok(Ok(_)) => {
                    process.finish_output().await;
                    return Ok(());
                }
                Ok(Err(error)) => {
                    return Err(SidecarError::CleanupFailed {
                        message: format!("could not reap child after graceful signal: {error}"),
                    });
                }
                Err(_) => {}
            }
        }

        self.diagnostics
            .push(DiagnosticKind::ForceKill, None, 0, None);
        if let Err(error) = process.child.start_kill() {
            if !matches!(process.child.try_wait(), Ok(Some(_))) {
                return Err(SidecarError::CleanupFailed {
                    message: format!("could not force-kill child: {error}"),
                });
            }
        }
        match timeout(self.config.force_kill_timeout, process.child.wait()).await {
            Ok(Ok(_)) => {
                process.finish_output().await;
                Ok(())
            }
            Ok(Err(error)) => Err(SidecarError::CleanupFailed {
                message: format!("could not reap force-killed child: {error}"),
            }),
            Err(_) => Err(SidecarError::CleanupFailed {
                message: "force-killed child did not reap before timeout".into(),
            }),
        }
    }

    fn publish(
        &self,
        status: SidecarStatus,
        failure: Option<SidecarFailure>,
        port: Option<u16>,
        restart_count: u8,
    ) {
        self.snapshot.send_replace(SidecarSnapshot {
            status,
            failure,
            port,
            restart_count,
        });
    }

    fn publish_failure(
        &self,
        status: SidecarStatus,
        kind: SidecarFailureKind,
        message: impl Into<String>,
        exit_code: Option<i32>,
        port: Option<u16>,
        restart_count: u8,
    ) {
        self.publish(
            status,
            Some(SidecarFailure {
                kind,
                message: message.into(),
                exit_code,
            }),
            port,
            restart_count,
        );
    }

    fn publish_stopped(&self, port: Option<u16>, restart_count: u8) {
        self.publish(
            SidecarStatus::NotInstalled,
            Some(SidecarFailure {
                kind: SidecarFailureKind::Cancelled,
                message: "lifecycle stopped by the application".into(),
                exit_code: None,
            }),
            port,
            restart_count,
        );
    }

    fn publish_cleanup_failure(&self, message: String, port: Option<u16>, restart_count: u8) {
        self.publish_failure(
            SidecarStatus::Failed,
            SidecarFailureKind::Cleanup,
            message,
            None,
            port,
            restart_count,
        );
    }
}

enum StartupOutcome {
    Ready,
    Cancelled,
    TimedOut,
    Exited(ExitStatus),
}

enum MonitorOutcome {
    Cancelled,
    Exited(ExitStatus),
}

struct OwnedProcess {
    child: Child,
    stdout_task: Option<JoinHandle<()>>,
    stderr_task: Option<JoinHandle<()>>,
}

impl OwnedProcess {
    async fn finish_output(&mut self) {
        for task in [&mut self.stdout_task, &mut self.stderr_task] {
            if let Some(mut task) = task.take() {
                if timeout(Duration::from_millis(200), &mut task)
                    .await
                    .is_err()
                {
                    task.abort();
                    let _ = task.await;
                }
            }
        }
    }
}

struct Diagnostics {
    capacity: usize,
    sequence: AtomicU64,
    entries: StdMutex<VecDeque<SidecarDiagnostic>>,
}

impl Diagnostics {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            sequence: AtomicU64::new(0),
            entries: StdMutex::new(VecDeque::with_capacity(capacity)),
        }
    }

    fn push(
        &self,
        kind: DiagnosticKind,
        stream: Option<DiagnosticStream>,
        byte_count: usize,
        exit_code: Option<i32>,
    ) {
        let entry = SidecarDiagnostic {
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
            kind,
            stream,
            byte_count,
            exit_code,
        };
        let mut entries = self.entries.lock().expect("sidecar diagnostics poisoned");
        if entries.len() == self.capacity {
            entries.pop_front();
        }
        entries.push_back(entry);
    }

    fn snapshot(&self) -> Vec<SidecarDiagnostic> {
        self.entries
            .lock()
            .expect("sidecar diagnostics poisoned")
            .iter()
            .cloned()
            .collect()
    }
}

async fn drain_output<R: AsyncRead + Unpin>(
    mut reader: R,
    stream: DiagnosticStream,
    diagnostics: Arc<Diagnostics>,
    chunk_bytes: usize,
) {
    let mut buffer = vec![0_u8; chunk_bytes.min(DEFAULT_OUTPUT_CHUNK_BYTES)];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(count) => diagnostics.push(DiagnosticKind::ChildOutput, Some(stream), count, None),
        }
    }
}

fn select_ephemeral_loopback_port() -> std::io::Result<u16> {
    // Release because GigaSTT must bind it. A repeated preflight immediately
    // before each launch rejects a listener which appeared in the meantime.
    let listener = std::net::TcpListener::bind((LOOPBACK_HOST, 0))?;
    Ok(listener.local_addr()?.port())
}

fn ensure_loopback_port_available(port: u16) -> std::io::Result<()> {
    // Repeated for the automatic restart. Since GigaSTT accepts neither an
    // inherited listener nor a readiness nonce, a residual same-user race
    // remains after this listener is released and before the child binds.
    let listener = std::net::TcpListener::bind((LOOPBACK_HOST, port))?;
    drop(listener);
    Ok(())
}

fn server_arguments(model_dir: &Path, port: u16) -> Vec<std::ffi::OsString> {
    vec![
        "--offline".into(),
        "serve".into(),
        "--host".into(),
        LOOPBACK_HOST.into(),
        "--port".into(),
        port.to_string().into(),
        "--model-dir".into(),
        model_dir.as_os_str().to_owned(),
        "--model-variant".into(),
        "rnnt".into(),
        "--punctuation".into(),
        "on".into(),
        "--punct-model-dir".into(),
        model_dir.join("punct").into_os_string(),
        "--itn".into(),
        "on".into(),
        "--vad".into(),
        "--vad-model-dir".into(),
        model_dir.join("vad").into_os_string(),
        "--pool-size".into(),
        "1".into(),
        "--pool-min-size".into(),
        "1".into(),
        "--batch-pool-size".into(),
        "0".into(),
        "--enable-jobs".into(),
        "--shutdown-drain-secs".into(),
        SHUTDOWN_DRAIN_SECS.into(),
    ]
}

#[cfg(unix)]
fn request_graceful_termination(child: &mut Child) -> bool {
    let Some(pid) = child.id() else {
        return false;
    };
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    const SIGTERM: i32 = 15;
    // SAFETY: `pid` is the currently-owned child and SIGTERM is valid.
    unsafe { kill(pid as i32, SIGTERM) == 0 }
}

#[cfg(not(unix))]
fn request_graceful_termination(_child: &mut Child) -> bool {
    false
}

fn duration_millis(value: Duration) -> u64 {
    value.as_millis().min(u64::MAX as u128) as u64
}

fn is_gigastt_environment_name(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy()
        .get(.."GIGASTT_".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("GIGASTT_"))
}

#[cfg(test)]
mod tests {
    use super::is_gigastt_environment_name;

    #[test]
    fn gigastt_environment_prefix_is_ascii_case_insensitive_for_windows() {
        for name in ["GIGASTT_HOTWORDS", "gigastt_hotwords", "GigaStt_Profile"] {
            assert!(is_gigastt_environment_name(name.as_ref()));
        }
        assert!(!is_gigastt_environment_name("NOT_GIGASTT_PROFILE".as_ref()));
    }
}
