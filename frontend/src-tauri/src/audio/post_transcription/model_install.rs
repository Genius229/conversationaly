//! Explicit installation and verification of the pinned GigaSTT model set.
//!
//! Ordinary application startup never calls the network path in this module.
//! [`ModelInstaller::install`] is intended to be invoked only by the explicit
//! app-facing install/repair action.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// GigaSTT release whose model layout and hashes are pinned below.
pub const PINNED_MODEL_VERSION: &str = "2.18.0";

const PREQUANT_RELEASE_BASE: &str =
    "https://github.com/ekhodzitsky/gigastt/releases/download/models-v3-2026-06-22";

#[derive(Debug, Clone)]
struct PinnedAsset {
    relative_path: &'static str,
    url: &'static str,
    sha256: &'static str,
}

const PINNED_ASSETS: &[PinnedAsset] = &[
    PinnedAsset {
        relative_path: "v3_rnnt_encoder_int8.onnx",
        url: "https://github.com/ekhodzitsky/gigastt/releases/download/models-v3-2026-06-22/v3_rnnt_encoder_int8.onnx",
        sha256: "c52665e9d96c4ca3a153c063d2ee9af6c567fe2975ca50fd038b75bbf2f60e7f",
    },
    PinnedAsset {
        relative_path: "v3_rnnt_decoder.onnx",
        url: "https://github.com/ekhodzitsky/gigastt/releases/download/models-v3-2026-06-22/v3_rnnt_decoder.onnx",
        sha256: "443c3b7bd42b453611618135d6b1e7d9467e5dd97c8a68501da4aa355750c0da",
    },
    PinnedAsset {
        relative_path: "v3_rnnt_joint.onnx",
        url: "https://github.com/ekhodzitsky/gigastt/releases/download/models-v3-2026-06-22/v3_rnnt_joint.onnx",
        sha256: "fd1d02f45c2ad3d6b67cc149811ad794ab4b020ed49a0a9e2790a8619d1cddd8",
    },
    PinnedAsset {
        relative_path: "v3_vocab.txt",
        url: "https://github.com/ekhodzitsky/gigastt/releases/download/models-v3-2026-06-22/v3_vocab.txt",
        sha256: "a9143c30844d3c0bee3e9e927e4084774eb1b9eeaafc473b2c4521e4911a7c07",
    },
    PinnedAsset {
        relative_path: "punct/rupunct_small_int8.onnx",
        url: "https://huggingface.co/ekhodzitsky/rupunct-small-onnx/resolve/main/rupunct_small_int8.onnx",
        sha256: "b105da023474d98aa13ba18953ae67b04b17bd0595034bc06030c17536893933",
    },
    PinnedAsset {
        relative_path: "punct/tokenizer.json",
        url: "https://huggingface.co/ekhodzitsky/rupunct-small-onnx/resolve/main/tokenizer.json",
        sha256: "7ca617388c2092a3a84272025c52bbf3c6db0aee225c0351186295c0b5d3ddc6",
    },
    PinnedAsset {
        relative_path: "punct/config.json",
        url: "https://huggingface.co/ekhodzitsky/rupunct-small-onnx/resolve/main/config.json",
        sha256: "6924a8cf41ec2bd3a3aa73a387ae0ccd0aed253ec7cac4d2f53c7d27440891eb",
    },
    PinnedAsset {
        relative_path: "vad/silero_vad.onnx",
        url: "https://github.com/snakers4/silero-vad/raw/v5.1.2/src/silero_vad/data/silero_vad.onnx",
        sha256: "2623a2953f6ff3d2c1e61740c6cdb7168133479b267dfef114a4a3cc5bdd788f",
    },
];

const MAX_MODEL_FILE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone)]
struct ModelAsset {
    relative_path: String,
    url: String,
    sha256: String,
    max_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
enum SourcePolicy {
    Production,
    #[cfg(feature = "gigastt-test-fixtures")]
    TestLoopback,
}

#[cfg(feature = "gigastt-test-fixtures")]
#[derive(Debug, Clone)]
pub struct TestModelAsset {
    relative_path: String,
    url: String,
    sha256: String,
    max_bytes: u64,
}

#[cfg(feature = "gigastt-test-fixtures")]
impl TestModelAsset {
    pub fn new(
        relative_path: impl Into<String>,
        url: impl Into<String>,
        sha256: impl Into<String>,
    ) -> Self {
        Self {
            relative_path: relative_path.into(),
            url: url.into(),
            sha256: sha256.into(),
            max_bytes: MAX_MODEL_FILE_BYTES,
        }
    }

    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes;
        self
    }
}

#[derive(Debug, Clone)]
pub struct ModelInstaller {
    model_dir: PathBuf,
    assets: Vec<ModelAsset>,
    client: reqwest::Client,
    install_lock: Arc<Mutex<()>>,
}

impl ModelInstaller {
    pub fn new(model_dir: PathBuf) -> Result<Self, ModelInstallError> {
        if model_dir.as_os_str().is_empty() {
            return Err(ModelInstallError::InvalidManifest {
                reason: "model root must not be empty".into(),
            });
        }
        debug_assert!(PINNED_ASSETS
            .iter()
            .take(4)
            .all(|asset| asset.url.starts_with(PREQUANT_RELEASE_BASE)));
        let assets = PINNED_ASSETS
            .iter()
            .map(|asset| ModelAsset {
                relative_path: asset.relative_path.to_owned(),
                url: asset.url.to_owned(),
                sha256: asset.sha256.to_owned(),
                max_bytes: MAX_MODEL_FILE_BYTES,
            })
            .collect();
        Self::build(model_dir, assets, SourcePolicy::Production)
    }

    #[cfg(feature = "gigastt-test-fixtures")]
    pub fn for_test(
        model_dir: PathBuf,
        assets: Vec<TestModelAsset>,
    ) -> Result<Self, ModelInstallError> {
        let assets = assets
            .into_iter()
            .map(|asset| ModelAsset {
                relative_path: asset.relative_path,
                url: asset.url,
                sha256: asset.sha256,
                max_bytes: asset.max_bytes,
            })
            .collect();
        Self::build(model_dir, assets, SourcePolicy::TestLoopback)
    }

    fn build(
        model_dir: PathBuf,
        assets: Vec<ModelAsset>,
        source_policy: SourcePolicy,
    ) -> Result<Self, ModelInstallError> {
        if model_dir.as_os_str().is_empty() {
            return Err(ModelInstallError::InvalidManifest {
                reason: "model root must not be empty".into(),
            });
        }
        validate_manifest(&assets, source_policy)?;
        let client = build_http_client(source_policy)?;
        Ok(Self {
            model_dir,
            assets,
            client,
            install_lock: Arc::new(Mutex::new(())),
        })
    }

    pub async fn status(&self) -> Result<ModelAvailability, ModelInstallError> {
        self.verify().await
    }

    pub async fn verify(&self) -> Result<ModelAvailability, ModelInstallError> {
        self.verify_with_cancel(CancellationToken::new()).await
    }

    async fn verify_with_cancel(
        &self,
        cancel: CancellationToken,
    ) -> Result<ModelAvailability, ModelInstallError> {
        let mut files = Vec::with_capacity(self.assets.len());
        for asset in &self.assets {
            ensure_not_cancelled(&cancel, Some(&asset.relative_path))?;
            let state = if let Some(path) =
                inspect_existing_path(&self.model_dir, &asset.relative_path).await?
            {
                let actual_sha256 =
                    hash_file_cancellable(path, &asset.relative_path, cancel.clone()).await?;
                if actual_sha256 == asset.sha256 {
                    ModelFileState::Valid
                } else {
                    ModelFileState::Corrupt { actual_sha256 }
                }
            } else {
                ModelFileState::Missing
            };
            files.push(ModelFileAvailability {
                relative_path: asset.relative_path.clone(),
                state,
            });
        }

        Ok(ModelAvailability {
            version: PINNED_MODEL_VERSION.to_owned(),
            files,
        })
    }

    pub async fn install<S>(
        &self,
        cancel: CancellationToken,
        progress: S,
    ) -> Result<ModelAvailability, ModelInstallError>
    where
        S: ModelInstallProgressSink,
    {
        let _guard = tokio::select! {
            _ = cancel.cancelled() => return Err(cancelled(None)),
            guard = self.install_lock.lock() => guard,
        };
        ensure_not_cancelled(&cancel, None)?;

        let initial = self.verify_with_cancel(cancel.clone()).await?;
        let file_count = self.assets.len();
        for (index, (asset, availability)) in
            self.assets.iter().zip(initial.files().iter()).enumerate()
        {
            let file_index = index + 1;
            progress.emit(ModelInstallProgress {
                phase: ModelInstallPhase::Checking,
                current_file: Some(asset.relative_path.clone()),
                file_index,
                file_count,
                bytes_done: 0,
                bytes_total: None,
            });
            ensure_not_cancelled(&cancel, Some(&asset.relative_path))?;
            if availability.state == ModelFileState::Valid {
                progress.emit(ModelInstallProgress {
                    phase: ModelInstallPhase::Reused,
                    current_file: Some(asset.relative_path.clone()),
                    file_index,
                    file_count,
                    bytes_done: 0,
                    bytes_total: None,
                });
                continue;
            }
            self.download_asset(asset, file_index, file_count, &cancel, &progress)
                .await?;
        }

        ensure_not_cancelled(&cancel, None)?;
        let availability = self.verify_with_cancel(cancel.clone()).await?;
        if !availability.is_ready() {
            return Err(ModelInstallError::IncompleteAfterInstall);
        }
        progress.emit(ModelInstallProgress {
            phase: ModelInstallPhase::Complete,
            current_file: None,
            file_index: file_count,
            file_count,
            bytes_done: 0,
            bytes_total: None,
        });
        Ok(availability)
    }

    async fn download_asset<S>(
        &self,
        asset: &ModelAsset,
        file_index: usize,
        file_count: usize,
        cancel: &CancellationToken,
        progress: &S,
    ) -> Result<(), ModelInstallError>
    where
        S: ModelInstallProgressSink,
    {
        let final_path = prepare_destination(&self.model_dir, &asset.relative_path).await?;
        let partial_path = partial_path(&final_path);
        remove_stale_partial(&partial_path, &asset.relative_path).await?;

        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(cancelled(Some(&asset.relative_path))),
            response = self.client.get(&asset.url).send() => response,
        }
        .map_err(|error| ModelInstallError::Network {
            file: asset.relative_path.clone(),
            message: bounded_message(error),
        })?;
        if !response.status().is_success() {
            return Err(ModelInstallError::HttpStatus {
                file: asset.relative_path.clone(),
                status: response.status().as_u16(),
            });
        }
        let bytes_total = response.content_length();
        if bytes_total.is_some_and(|total| total > asset.max_bytes) {
            return Err(ModelInstallError::DownloadTooLarge {
                file: asset.relative_path.clone(),
                limit: asset.max_bytes,
                received: bytes_total.unwrap_or_default(),
            });
        }

        let cleanup = PartialFileGuard::new(partial_path.clone());
        let mut file = tokio::fs::File::create(&partial_path)
            .await
            .map_err(|error| disk_error(&asset.relative_path, error))?;
        let mut stream = response.bytes_stream();
        let mut bytes_done = 0_u64;
        let mut last_progress = Instant::now();
        progress.emit(download_progress(
            asset,
            file_index,
            file_count,
            bytes_done,
            bytes_total,
        ));

        loop {
            let next = tokio::select! {
                _ = cancel.cancelled() => return Err(cancelled(Some(&asset.relative_path))),
                next = stream.next() => next,
            };
            let Some(chunk) = next else {
                break;
            };
            let chunk = chunk.map_err(|error| ModelInstallError::Network {
                file: asset.relative_path.clone(),
                message: bounded_message(error),
            })?;
            let received = bytes_done.saturating_add(chunk.len() as u64);
            if received > asset.max_bytes {
                return Err(ModelInstallError::DownloadTooLarge {
                    file: asset.relative_path.clone(),
                    limit: asset.max_bytes,
                    received,
                });
            }
            tokio::select! {
                _ = cancel.cancelled() => return Err(cancelled(Some(&asset.relative_path))),
                result = file.write_all(&chunk) => {
                    result.map_err(|error| disk_error(&asset.relative_path, error))?;
                }
            }
            bytes_done = received;
            if last_progress.elapsed() >= PROGRESS_INTERVAL {
                progress.emit(download_progress(
                    asset,
                    file_index,
                    file_count,
                    bytes_done,
                    bytes_total,
                ));
                last_progress = Instant::now();
            }
        }
        tokio::select! {
            _ = cancel.cancelled() => return Err(cancelled(Some(&asset.relative_path))),
            result = file.flush() => {
                result.map_err(|error| disk_error(&asset.relative_path, error))?;
            }
        }
        tokio::select! {
            _ = cancel.cancelled() => return Err(cancelled(Some(&asset.relative_path))),
            result = file.sync_all() => {
                result.map_err(|error| disk_error(&asset.relative_path, error))?;
            }
        }
        drop(file);
        progress.emit(download_progress(
            asset,
            file_index,
            file_count,
            bytes_done,
            bytes_total,
        ));
        progress.emit(ModelInstallProgress {
            phase: ModelInstallPhase::Verifying,
            current_file: Some(asset.relative_path.clone()),
            file_index,
            file_count,
            bytes_done,
            bytes_total,
        });

        let actual =
            hash_file_cancellable(partial_path.clone(), &asset.relative_path, cancel.clone())
                .await?;
        if actual != asset.sha256 {
            return Err(ModelInstallError::ChecksumMismatch {
                file: asset.relative_path.clone(),
                expected: asset.sha256.clone(),
                actual,
            });
        }
        ensure_not_cancelled(cancel, Some(&asset.relative_path))?;
        promote_partial(&partial_path, &final_path, &asset.relative_path).await?;
        cleanup.disarm();
        Ok(())
    }
}

pub trait ModelInstallProgressSink: Send + Sync {
    fn emit(&self, progress: ModelInstallProgress);
}

impl<F> ModelInstallProgressSink for F
where
    F: Fn(ModelInstallProgress) + Send + Sync,
{
    fn emit(&self, progress: ModelInstallProgress) {
        self(progress);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelInstallPhase {
    Checking,
    Reused,
    Downloading,
    Verifying,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInstallProgress {
    pub phase: ModelInstallPhase,
    pub current_file: Option<String>,
    pub file_index: usize,
    pub file_count: usize,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
}

fn validate_manifest(
    assets: &[ModelAsset],
    source_policy: SourcePolicy,
) -> Result<(), ModelInstallError> {
    if assets.is_empty() {
        return Err(ModelInstallError::InvalidManifest {
            reason: "at least one model file is required".into(),
        });
    }

    let mut paths = HashSet::with_capacity(assets.len());
    for asset in assets {
        validate_relative_path(&asset.relative_path)?;
        if !paths.insert(asset.relative_path.to_ascii_lowercase()) {
            return Err(ModelInstallError::InvalidManifest {
                reason: format!("duplicate model path: {}", asset.relative_path),
            });
        }
        if asset.sha256.len() != 64
            || !asset
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ModelInstallError::InvalidManifest {
                reason: format!("invalid SHA-256 for {}", asset.relative_path),
            });
        }
        if asset.max_bytes == 0 || asset.max_bytes > MAX_MODEL_FILE_BYTES {
            return Err(ModelInstallError::InvalidManifest {
                reason: format!("invalid download size limit for {}", asset.relative_path),
            });
        }
        validate_source_url(&asset.url, source_policy)?;
    }
    Ok(())
}

fn validate_relative_path(relative_path: &str) -> Result<(), ModelInstallError> {
    let path = Path::new(relative_path);
    let portable_segments = relative_path.split('/').all(|segment| {
        !segment.is_empty() && segment != "." && segment != ".." && !segment.contains(['\\', ':'])
    });
    if !portable_segments
        || path.is_absolute()
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(ModelInstallError::InvalidManifest {
            reason: format!("model path must be a portable relative path: {relative_path}"),
        });
    }
    Ok(())
}

fn validate_source_url(url: &str, source_policy: SourcePolicy) -> Result<(), ModelInstallError> {
    let parsed = reqwest::Url::parse(url).map_err(|_| ModelInstallError::InvalidManifest {
        reason: "model source URL is invalid".into(),
    })?;
    if !parsed.username().is_empty() || parsed.password().is_some() || parsed.fragment().is_some() {
        return Err(ModelInstallError::InvalidManifest {
            reason: "model source URL must not contain credentials or a fragment".into(),
        });
    }
    let host = parsed.host_str().unwrap_or_default();
    let allowed = match source_policy {
        SourcePolicy::Production => parsed.scheme() == "https" && is_approved_production_host(host),
        #[cfg(feature = "gigastt-test-fixtures")]
        SourcePolicy::TestLoopback => {
            matches!(parsed.scheme(), "http" | "https")
                && (host.eq_ignore_ascii_case("localhost")
                    || host
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|address| address.is_loopback()))
        }
    };
    if !allowed {
        return Err(ModelInstallError::InvalidManifest {
            reason: "model source URL is not approved".into(),
        });
    }
    Ok(())
}

fn is_approved_production_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("github.com")
        || host.eq_ignore_ascii_case("huggingface.co")
        || host.ends_with(".githubusercontent.com")
        || host.ends_with(".hf.co")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelAvailability {
    pub version: String,
    files: Vec<ModelFileAvailability>,
}

impl ModelAvailability {
    pub fn is_ready(&self) -> bool {
        self.files
            .iter()
            .all(|file| file.state == ModelFileState::Valid)
    }

    pub fn files(&self) -> &[ModelFileAvailability] {
        &self.files
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelFileAvailability {
    pub relative_path: String,
    #[serde(flatten)]
    pub state: ModelFileState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ModelFileState {
    Valid,
    Missing,
    Corrupt { actual_sha256: String },
}

impl ModelFileState {
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}

#[derive(Debug, Error)]
pub enum ModelInstallError {
    #[error("invalid GigaSTT model manifest: {reason}")]
    InvalidManifest { reason: String },
    #[error("could not verify GigaSTT model file {file}: {message}")]
    Verification { file: String, message: String },
    #[error("GigaSTT model installation was cancelled")]
    Cancelled { file: Option<String> },
    #[error("could not download GigaSTT model file {file}: {message}")]
    Network { file: String, message: String },
    #[error("could not download GigaSTT model file {file}: HTTP {status}")]
    HttpStatus { file: String, status: u16 },
    #[error(
        "GigaSTT model file {file} exceeded its {limit}-byte download limit ({received} bytes)"
    )]
    DownloadTooLarge {
        file: String,
        limit: u64,
        received: u64,
    },
    #[error("could not write GigaSTT model file {file}: {message}")]
    Disk { file: String, message: String },
    #[error("SHA-256 mismatch for GigaSTT model file {file}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        file: String,
        expected: String,
        actual: String,
    },
    #[error("could not atomically install GigaSTT model file {file}: {message}")]
    Finalize { file: String, message: String },
    #[error("unsafe path inside the GigaSTT model root: {path}")]
    UnsafePath { path: PathBuf },
    #[error("GigaSTT model installation finished without all required files")]
    IncompleteAfterInstall,
}

fn build_http_client(source_policy: SourcePolicy) -> Result<reqwest::Client, ModelInstallError> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                attempt.error("too many model download redirects")
            } else if source_url_is_allowed(attempt.url(), source_policy) {
                attempt.follow()
            } else {
                attempt.error("model download redirect host is not approved")
            }
        }))
        .build()
        .map_err(|error| ModelInstallError::InvalidManifest {
            reason: format!("could not construct model download client: {error}"),
        })
}

fn source_url_is_allowed(url: &reqwest::Url, source_policy: SourcePolicy) -> bool {
    let host = url.host_str().unwrap_or_default();
    match source_policy {
        SourcePolicy::Production => url.scheme() == "https" && is_approved_production_host(host),
        #[cfg(feature = "gigastt-test-fixtures")]
        SourcePolicy::TestLoopback => {
            matches!(url.scheme(), "http" | "https")
                && (host.eq_ignore_ascii_case("localhost")
                    || host
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|address| address.is_loopback()))
        }
    }
}

fn download_progress(
    asset: &ModelAsset,
    file_index: usize,
    file_count: usize,
    bytes_done: u64,
    bytes_total: Option<u64>,
) -> ModelInstallProgress {
    ModelInstallProgress {
        phase: ModelInstallPhase::Downloading,
        current_file: Some(asset.relative_path.clone()),
        file_index,
        file_count,
        bytes_done,
        bytes_total,
    }
}

async fn inspect_existing_path(
    model_dir: &Path,
    relative_path: &str,
) -> Result<Option<PathBuf>, ModelInstallError> {
    let root_metadata = match tokio::fs::symlink_metadata(model_dir).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(disk_error(relative_path, error)),
    };
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(ModelInstallError::UnsafePath {
            path: model_dir.to_path_buf(),
        });
    }
    let canonical_root = tokio::fs::canonicalize(model_dir)
        .await
        .map_err(|error| disk_error(relative_path, error))?;
    let final_path = model_dir.join(relative_path);
    let relative_parent =
        Path::new(relative_path)
            .parent()
            .ok_or_else(|| ModelInstallError::InvalidManifest {
                reason: format!("model path has no parent: {relative_path}"),
            })?;
    let mut current = model_dir.to_path_buf();
    for component in relative_parent.components() {
        let Component::Normal(component) = component else {
            return Err(ModelInstallError::UnsafePath { path: current });
        };
        current.push(component);
        let metadata = match tokio::fs::symlink_metadata(&current).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(disk_error(relative_path, error)),
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ModelInstallError::UnsafePath {
                path: current.clone(),
            });
        }
        let canonical = tokio::fs::canonicalize(&current)
            .await
            .map_err(|error| disk_error(relative_path, error))?;
        if !canonical.starts_with(&canonical_root) {
            return Err(ModelInstallError::UnsafePath {
                path: current.clone(),
            });
        }
    }

    match tokio::fs::symlink_metadata(&final_path).await {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(ModelInstallError::UnsafePath { path: final_path })
        }
        Ok(_) => Ok(Some(final_path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(disk_error(relative_path, error)),
    }
}

async fn prepare_destination(
    model_dir: &Path,
    relative_path: &str,
) -> Result<PathBuf, ModelInstallError> {
    tokio::fs::create_dir_all(model_dir)
        .await
        .map_err(|error| disk_error(relative_path, error))?;
    let root_metadata = tokio::fs::symlink_metadata(model_dir)
        .await
        .map_err(|error| disk_error(relative_path, error))?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(ModelInstallError::UnsafePath {
            path: model_dir.to_path_buf(),
        });
    }
    let canonical_root = tokio::fs::canonicalize(model_dir)
        .await
        .map_err(|error| disk_error(relative_path, error))?;

    let final_path = model_dir.join(relative_path);
    let relative_parent =
        Path::new(relative_path)
            .parent()
            .ok_or_else(|| ModelInstallError::InvalidManifest {
                reason: format!("model path has no parent: {relative_path}"),
            })?;
    let mut current = model_dir.to_path_buf();
    for component in relative_parent.components() {
        let Component::Normal(component) = component else {
            return Err(ModelInstallError::UnsafePath { path: current });
        };
        current.push(component);
        match tokio::fs::symlink_metadata(&current).await {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(ModelInstallError::UnsafePath {
                        path: current.clone(),
                    });
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match tokio::fs::create_dir(&current).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(disk_error(relative_path, error)),
                }
                let metadata = tokio::fs::symlink_metadata(&current)
                    .await
                    .map_err(|error| disk_error(relative_path, error))?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(ModelInstallError::UnsafePath {
                        path: current.clone(),
                    });
                }
            }
            Err(error) => return Err(disk_error(relative_path, error)),
        }
        let canonical = tokio::fs::canonicalize(&current)
            .await
            .map_err(|error| disk_error(relative_path, error))?;
        if !canonical.starts_with(&canonical_root) {
            return Err(ModelInstallError::UnsafePath {
                path: current.clone(),
            });
        }
    }

    match tokio::fs::symlink_metadata(&final_path).await {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(ModelInstallError::UnsafePath { path: final_path });
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(disk_error(relative_path, error)),
    }
    Ok(final_path)
}

fn partial_path(final_path: &Path) -> PathBuf {
    let mut path = final_path.as_os_str().to_owned();
    path.push(".partial");
    PathBuf::from(path)
}

async fn remove_stale_partial(path: &Path, label: &str) -> Result<(), ModelInstallError> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(disk_error(label, error)),
    }
}

struct PartialFileGuard(Option<PathBuf>);

impl PartialFileGuard {
    fn new(path: PathBuf) -> Self {
        Self(Some(path))
    }

    fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for PartialFileGuard {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

async fn promote_partial(
    partial: &Path,
    final_path: &Path,
    label: &str,
) -> Result<(), ModelInstallError> {
    // `rename` maps to a same-volume atomic replacement on supported desktop
    // platforms (MoveFileExW with REPLACE_EXISTING on Windows). The verified
    // partial is the source, so a failed call leaves the prior final untouched.
    tokio::fs::rename(partial, final_path)
        .await
        .map_err(|error| finalize_error(label, error))
}

fn ensure_not_cancelled(
    cancel: &CancellationToken,
    file: Option<&str>,
) -> Result<(), ModelInstallError> {
    if cancel.is_cancelled() {
        Err(cancelled(file))
    } else {
        Ok(())
    }
}

fn cancelled(file: Option<&str>) -> ModelInstallError {
    ModelInstallError::Cancelled {
        file: file.map(ToOwned::to_owned),
    }
}

fn disk_error(file: &str, error: std::io::Error) -> ModelInstallError {
    ModelInstallError::Disk {
        file: file.to_owned(),
        message: bounded_message(error),
    }
}

fn finalize_error(file: &str, error: std::io::Error) -> ModelInstallError {
    ModelInstallError::Finalize {
        file: file.to_owned(),
        message: bounded_message(error),
    }
}

fn bounded_message(error: impl std::fmt::Display) -> String {
    error.to_string().chars().take(512).collect()
}

async fn hash_file_cancellable(
    path: PathBuf,
    label: &str,
    cancel: CancellationToken,
) -> Result<String, ModelInstallError> {
    let label = label.to_owned();
    tokio::task::spawn_blocking(move || hash_file_blocking(&path, &cancel))
        .await
        .map_err(|error| ModelInstallError::Verification {
            file: label.clone(),
            message: bounded_message(error),
        })?
        .map_err(|error| match error {
            BlockingHashError::Cancelled => cancelled(Some(&label)),
            BlockingHashError::Io(error) => ModelInstallError::Verification {
                file: label,
                message: bounded_message(error),
            },
        })
}

enum BlockingHashError {
    Cancelled,
    Io(std::io::Error),
}

impl From<std::io::Error> for BlockingHashError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

fn hash_file_blocking(
    path: &Path,
    cancel: &CancellationToken,
) -> Result<String, BlockingHashError> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if cancel.is_cancelled() {
            return Err(BlockingHashError::Cancelled);
        }
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
