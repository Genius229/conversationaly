use app_lib::audio::post_transcription::model_install::{
    ModelFileState, ModelInstallError, ModelInstallPhase, ModelInstallProgress, ModelInstaller,
    TestModelAsset,
};
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy)]
enum ResponseMode {
    ContentLength,
    Chunked { delay: Duration },
    Status { code: u16 },
    RedirectExternal,
}

struct TestServer {
    url: String,
    requests: Arc<AtomicUsize>,
    shutdown: CancellationToken,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

async fn serve(body: &[u8], mode: ResponseMode) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let request_count = requests.clone();
    let body = body.to_vec();
    let shutdown = CancellationToken::new();
    let stop = shutdown.clone();
    tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                _ = stop.cancelled() => break,
                accepted = listener.accept() => accepted,
            };
            let Ok((mut stream, _)) = accepted else {
                break;
            };
            request_count.fetch_add(1, Ordering::SeqCst);
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let Ok(read) = stream.read(&mut buffer).await else {
                    break;
                };
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            match mode {
                ResponseMode::ContentLength => {
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(header.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                }
                ResponseMode::Chunked { delay } => {
                    let _ = stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                        )
                        .await;
                    for chunk in body.chunks(2) {
                        let header = format!("{:x}\r\n", chunk.len());
                        if stream.write_all(header.as_bytes()).await.is_err()
                            || stream.write_all(chunk).await.is_err()
                            || stream.write_all(b"\r\n").await.is_err()
                        {
                            break;
                        }
                        tokio::time::sleep(delay).await;
                    }
                    let _ = stream.write_all(b"0\r\n\r\n").await;
                }
                ResponseMode::Status { code } => {
                    let header = format!(
                        "HTTP/1.1 {code} failure\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(header.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                }
                ResponseMode::RedirectExternal => {
                    let _ = stream
                        .write_all(
                            b"HTTP/1.1 302 Found\r\nLocation: https://example.com/unapproved\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .await;
                }
            }
        }
    });
    TestServer {
        url: format!("http://{address}/model"),
        requests,
        shutdown,
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[tokio::test]
async fn pinned_manifest_requires_every_main_punctuation_and_vad_file() {
    let root = tempfile::tempdir().unwrap();
    let installer = ModelInstaller::new(root.path().to_path_buf()).unwrap();

    let availability = installer.status().await.unwrap();

    assert!(!availability.is_ready());
    let missing: Vec<_> = availability
        .files()
        .iter()
        .filter(|file| file.state.is_missing())
        .map(|file| file.relative_path.as_str())
        .collect();
    assert_eq!(
        missing,
        [
            "v3_rnnt_encoder_int8.onnx",
            "v3_rnnt_decoder.onnx",
            "v3_rnnt_joint.onnx",
            "v3_vocab.txt",
            "punct/rupunct_small_int8.onnx",
            "punct/tokenizer.json",
            "punct/config.json",
            "vad/silero_vad.onnx",
        ]
    );
}

#[test]
fn test_manifest_rejects_paths_that_can_escape_the_model_root() {
    let root = tempfile::tempdir().unwrap();
    for path in [
        "../escape.onnx",
        "/absolute.onnx",
        "punct/../../escape.onnx",
        r"punct\..\escape.onnx",
        "C:/escape.onnx",
    ] {
        let error = ModelInstaller::for_test(
            root.path().to_path_buf(),
            vec![TestModelAsset::new(
                path,
                "http://127.0.0.1:9/model",
                "0000000000000000000000000000000000000000000000000000000000000000",
            )],
        )
        .unwrap_err();
        assert!(
            matches!(error, ModelInstallError::InvalidManifest { .. }),
            "path {path:?} produced {error:?}"
        );
    }
}

#[test]
fn test_manifest_does_not_turn_arbitrary_urls_into_production_sources() {
    let root = tempfile::tempdir().unwrap();
    for url in [
        "http://example.com/model",
        "https://example.com/model",
        "file:///tmp/model",
    ] {
        let error = ModelInstaller::for_test(
            root.path().to_path_buf(),
            vec![TestModelAsset::new(
                "model.onnx",
                url,
                "0000000000000000000000000000000000000000000000000000000000000000",
            )],
        )
        .unwrap_err();
        assert!(
            matches!(error, ModelInstallError::InvalidManifest { .. }),
            "URL {url:?} produced {error:?}"
        );
    }
}

#[tokio::test]
async fn verify_hashes_existing_files_instead_of_trusting_presence() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("v3_rnnt_encoder_int8.onnx"), b"corrupt").unwrap();
    let installer = ModelInstaller::new(root.path().to_path_buf()).unwrap();

    let availability = installer.verify().await.unwrap();

    assert!(!availability.is_ready());
    assert!(matches!(
        &availability.files()[0].state,
        ModelFileState::Corrupt { actual_sha256 }
            if actual_sha256 == "11d510e067d2cdcd7559bd86d27a2f4c20babd43670346b97af99b522c1f0075"
    ));
}

#[tokio::test]
async fn install_streams_unknown_length_verifies_and_reports_exact_progress() {
    let root = tempfile::tempdir().unwrap();
    let bytes = b"verified-model";
    let server = serve(
        bytes,
        ResponseMode::Chunked {
            delay: Duration::from_millis(1),
        },
    )
    .await;
    let installer = ModelInstaller::for_test(
        root.path().to_path_buf(),
        vec![TestModelAsset::new(
            "vad/test.onnx",
            &server.url,
            sha256(bytes),
        )],
    )
    .unwrap();
    let progress = Arc::new(Mutex::new(Vec::new()));
    let captured = progress.clone();

    let availability = installer
        .install(CancellationToken::new(), move |event| {
            captured.lock().unwrap().push(event);
        })
        .await
        .unwrap();

    assert!(availability.is_ready());
    assert_eq!(
        std::fs::read(root.path().join("vad/test.onnx")).unwrap(),
        bytes
    );
    assert_eq!(server.requests.load(Ordering::SeqCst), 1);
    assert!(!root.path().join("vad/test.onnx.partial").exists());
    let progress = progress.lock().unwrap();
    let downloads: Vec<_> = progress
        .iter()
        .filter(|event| event.phase == ModelInstallPhase::Downloading)
        .collect();
    assert!(!downloads.is_empty());
    assert!(downloads.iter().all(|event| {
        event.current_file.as_deref() == Some("vad/test.onnx") && event.bytes_total.is_none()
    }));
    assert_eq!(downloads.last().unwrap().bytes_done, bytes.len() as u64);
    assert_eq!(progress.last().unwrap().phase, ModelInstallPhase::Complete);
}

#[tokio::test]
async fn install_reuses_a_cached_file_only_after_its_hash_matches() {
    let root = tempfile::tempdir().unwrap();
    let bytes = b"cached-valid-model";
    std::fs::write(root.path().join("model.onnx"), bytes).unwrap();
    let server = serve(bytes, ResponseMode::ContentLength).await;
    let installer = ModelInstaller::for_test(
        root.path().to_path_buf(),
        vec![TestModelAsset::new(
            "model.onnx",
            &server.url,
            sha256(bytes),
        )],
    )
    .unwrap();
    let progress = Arc::new(Mutex::new(Vec::new()));
    let captured = progress.clone();

    let availability = installer
        .install(CancellationToken::new(), move |event| {
            captured.lock().unwrap().push(event);
        })
        .await
        .unwrap();

    assert!(availability.is_ready());
    assert_eq!(server.requests.load(Ordering::SeqCst), 0);
    assert!(progress
        .lock()
        .unwrap()
        .iter()
        .any(|event| event.phase == ModelInstallPhase::Reused));
}

#[tokio::test]
async fn hash_mismatch_removes_only_the_partial_and_keeps_the_existing_file() {
    let root = tempfile::tempdir().unwrap();
    let old = b"existing-corrupt-model";
    let downloaded = b"also-not-the-expected-model";
    std::fs::write(root.path().join("model.onnx"), old).unwrap();
    let server = serve(downloaded, ResponseMode::ContentLength).await;
    let installer = ModelInstaller::for_test(
        root.path().to_path_buf(),
        vec![TestModelAsset::new(
            "model.onnx",
            &server.url,
            sha256(b"expected-model"),
        )],
    )
    .unwrap();

    let error = installer
        .install(CancellationToken::new(), |_| {})
        .await
        .unwrap_err();

    assert!(matches!(error, ModelInstallError::ChecksumMismatch { .. }));
    assert_eq!(std::fs::read(root.path().join("model.onnx")).unwrap(), old);
    assert!(!root.path().join("model.onnx.partial").exists());
}

#[tokio::test]
async fn cancellation_removes_the_partial_and_keeps_the_existing_file() {
    let root = tempfile::tempdir().unwrap();
    let old = b"existing-corrupt-model";
    std::fs::write(root.path().join("model.onnx"), old).unwrap();
    let downloaded = vec![b'x'; 128];
    let server = serve(
        &downloaded,
        ResponseMode::Chunked {
            delay: Duration::from_millis(60),
        },
    )
    .await;
    let installer = ModelInstaller::for_test(
        root.path().to_path_buf(),
        vec![TestModelAsset::new(
            "model.onnx",
            &server.url,
            sha256(&downloaded),
        )],
    )
    .unwrap();
    let cancel = CancellationToken::new();
    let cancel_from_progress = cancel.clone();

    let error = installer
        .install(cancel, move |event: ModelInstallProgress| {
            if event.phase == ModelInstallPhase::Downloading && event.bytes_done > 0 {
                cancel_from_progress.cancel();
            }
        })
        .await
        .unwrap_err();

    assert!(matches!(error, ModelInstallError::Cancelled { .. }));
    assert_eq!(std::fs::read(root.path().join("model.onnx")).unwrap(), old);
    assert!(!root.path().join("model.onnx.partial").exists());
}

#[tokio::test]
async fn explicit_install_repairs_a_corrupt_file_and_discards_stale_partial() {
    let root = tempfile::tempdir().unwrap();
    let bytes = b"replacement-model";
    std::fs::write(root.path().join("model.onnx"), b"corrupt").unwrap();
    std::fs::write(root.path().join("model.onnx.partial"), b"stale").unwrap();
    let server = serve(bytes, ResponseMode::ContentLength).await;
    let installer = ModelInstaller::for_test(
        root.path().to_path_buf(),
        vec![TestModelAsset::new(
            "model.onnx",
            &server.url,
            sha256(bytes),
        )],
    )
    .unwrap();
    let progress = Arc::new(Mutex::new(Vec::new()));
    let captured = progress.clone();

    let availability = installer
        .install(CancellationToken::new(), move |event| {
            captured.lock().unwrap().push(event);
        })
        .await
        .unwrap();

    assert!(availability.is_ready());
    assert_eq!(
        std::fs::read(root.path().join("model.onnx")).unwrap(),
        bytes
    );
    assert!(!root.path().join("model.onnx.partial").exists());
    let progress = progress.lock().unwrap();
    let final_download = progress
        .iter()
        .rev()
        .find(|event| event.phase == ModelInstallPhase::Downloading)
        .unwrap();
    assert_eq!(final_download.bytes_done, bytes.len() as u64);
    assert_eq!(final_download.bytes_total, Some(bytes.len() as u64));
}

#[tokio::test]
async fn response_size_limit_is_enforced_before_writing() {
    let root = tempfile::tempdir().unwrap();
    let bytes = b"larger-than-four-bytes";
    let server = serve(bytes, ResponseMode::ContentLength).await;
    let installer = ModelInstaller::for_test(
        root.path().to_path_buf(),
        vec![TestModelAsset::new("model.onnx", &server.url, sha256(bytes)).with_max_bytes(4)],
    )
    .unwrap();

    let error = installer
        .install(CancellationToken::new(), |_| {})
        .await
        .unwrap_err();

    assert!(matches!(error, ModelInstallError::DownloadTooLarge { .. }));
    assert!(!root.path().join("model.onnx").exists());
    assert!(!root.path().join("model.onnx.partial").exists());
}

#[tokio::test]
async fn http_errors_are_typed_without_exposing_the_response_body() {
    let root = tempfile::tempdir().unwrap();
    let server = serve(b"secret upstream body", ResponseMode::Status { code: 503 }).await;
    let installer = ModelInstaller::for_test(
        root.path().to_path_buf(),
        vec![TestModelAsset::new(
            "model.onnx",
            &server.url,
            sha256(b"unused"),
        )],
    )
    .unwrap();

    let error = installer
        .install(CancellationToken::new(), |_| {})
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        ModelInstallError::HttpStatus { status: 503, .. }
    ));
    assert!(!error.to_string().contains("secret upstream body"));
}

#[tokio::test]
async fn redirect_to_an_unapproved_host_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    let server = serve(b"", ResponseMode::RedirectExternal).await;
    let installer = ModelInstaller::for_test(
        root.path().to_path_buf(),
        vec![TestModelAsset::new(
            "model.onnx",
            &server.url,
            sha256(b"unused"),
        )],
    )
    .unwrap();

    let error = installer
        .install(CancellationToken::new(), |_| {})
        .await
        .unwrap_err();

    assert!(matches!(error, ModelInstallError::Network { .. }));
    assert_eq!(server.requests.load(Ordering::SeqCst), 1);
    assert!(!root.path().join("model.onnx").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn install_rejects_a_symlink_that_would_escape_the_model_root() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("punct")).unwrap();
    let bytes = b"must-stay-inside";
    let server = serve(bytes, ResponseMode::ContentLength).await;
    let installer = ModelInstaller::for_test(
        root.path().to_path_buf(),
        vec![TestModelAsset::new(
            "punct/model.onnx",
            &server.url,
            sha256(bytes),
        )],
    )
    .unwrap();

    let error = installer
        .install(CancellationToken::new(), |_| {})
        .await
        .unwrap_err();

    assert!(matches!(error, ModelInstallError::UnsafePath { .. }));
    assert!(!outside.path().join("model.onnx").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn verify_never_accepts_a_symlinked_model_file_as_available() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let bytes = b"valid-but-outside";
    let outside_model = outside.path().join("model.onnx");
    std::fs::write(&outside_model, bytes).unwrap();
    std::os::unix::fs::symlink(&outside_model, root.path().join("model.onnx")).unwrap();
    let installer = ModelInstaller::for_test(
        root.path().to_path_buf(),
        vec![TestModelAsset::new(
            "model.onnx",
            "http://127.0.0.1:9/model",
            sha256(bytes),
        )],
    )
    .unwrap();

    let error = installer.verify().await.unwrap_err();

    assert!(matches!(error, ModelInstallError::UnsafePath { .. }));
}

#[tokio::test]
async fn download_progress_is_throttled_but_always_finishes_with_exact_bytes() {
    let root = tempfile::tempdir().unwrap();
    let bytes = vec![b'p'; 240];
    let server = serve(
        &bytes,
        ResponseMode::Chunked {
            delay: Duration::from_millis(5),
        },
    )
    .await;
    let installer = ModelInstaller::for_test(
        root.path().to_path_buf(),
        vec![TestModelAsset::new(
            "model.onnx",
            &server.url,
            sha256(&bytes),
        )],
    )
    .unwrap();
    let progress = Arc::new(Mutex::new(Vec::new()));
    let captured = progress.clone();

    installer
        .install(CancellationToken::new(), move |event| {
            captured.lock().unwrap().push((Instant::now(), event));
        })
        .await
        .unwrap();

    let progress = progress.lock().unwrap();
    let downloads: Vec<_> = progress
        .iter()
        .filter(|(_, event)| event.phase == ModelInstallPhase::Downloading)
        .collect();
    assert!(
        downloads.len() >= 2,
        "expected initial and exact final download events"
    );
    assert_eq!(downloads.first().unwrap().1.bytes_done, 0);
    let elapsed = downloads
        .last()
        .unwrap()
        .0
        .duration_since(downloads.first().unwrap().0);
    let throttle_interval = Duration::from_millis(100);
    let timed_slots = elapsed.as_nanos().div_ceil(throttle_interval.as_nanos());
    let max_events = usize::try_from(timed_slots)
        .unwrap_or(usize::MAX)
        .saturating_add(2); // Initial and mandatory exact final events are unthrottled.
    assert!(
        downloads.len() <= max_events,
        "{} download events over {elapsed:?}, maximum {max_events}",
        downloads.len(),
    );
    for pair in downloads[1..downloads.len() - 1].windows(2) {
        assert!(
            pair[1].0.duration_since(pair[0].0) >= throttle_interval,
            "intermediate download events were emitted less than {throttle_interval:?} apart"
        );
    }
    assert_eq!(downloads.last().unwrap().1.bytes_done, bytes.len() as u64);
    assert!(downloads.last().unwrap().1.bytes_total.is_none());
}
