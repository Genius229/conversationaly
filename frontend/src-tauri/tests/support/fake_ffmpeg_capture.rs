//! Process-level fake for the owned DirectShow capture contract tests.
//!
//! Each test copies this executable into a private directory and places a JSON
//! configuration beside that copy. Production code receives only the copied
//! executable path and has no test-only environment or argument switches.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(default)]
struct Config {
    enumeration_stderr: String,
    enumeration_delay_ms: u64,
    initial_delay_ms: u64,
    initial_value: f32,
    initial_samples: usize,
    byte_fragments: Vec<usize>,
    wait_for_quit: bool,
    ignore_quit: bool,
    tail_value: f32,
    tail_samples: usize,
    stderr_bytes: usize,
    exit_code: i32,
    invalid_pcm: Option<String>,
    close_stdout_after_initial: bool,
    exit_delay_after_quit_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enumeration_stderr: concat!(
                "[dshow @ 00000001] DirectShow audio devices\n",
                "[dshow @ 00000001]  \"Lifecycle Microphone\" (audio)\n",
                "[dshow @ 00000001]    Alternative name \"@device_cm_test_audio\"\n",
            )
            .to_string(),
            enumeration_delay_ms: 0,
            initial_delay_ms: 0,
            initial_value: 0.25,
            initial_samples: 480,
            byte_fragments: Vec::new(),
            wait_for_quit: true,
            ignore_quit: false,
            tail_value: -0.5,
            tail_samples: 0,
            stderr_bytes: 0,
            exit_code: 0,
            invalid_pcm: None,
            close_stdout_after_initial: false,
            exit_delay_after_quit_ms: 0,
        }
    }
}

fn sidecar_path(executable: &Path, extension: &str) -> PathBuf {
    executable.with_extension(extension)
}

fn append(path: &Path, value: &str) {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open fake marker");
    writeln!(file, "{value}").expect("write fake marker");
    file.flush().expect("flush fake marker");
}

fn bytes(value: f32, samples: usize) -> Vec<u8> {
    let mut output = Vec::with_capacity(samples.saturating_mul(4));
    for _ in 0..samples {
        output.extend_from_slice(&value.to_le_bytes());
    }
    output
}

fn write_fragmented(mut output: impl Write, payload: &[u8], fragments: &[usize]) {
    if fragments.is_empty() {
        output.write_all(payload).expect("write fake PCM");
        output.flush().expect("flush fake PCM");
        return;
    }

    let mut offset = 0;
    let mut fragment = 0;
    while offset < payload.len() {
        let configured = fragments[fragment % fragments.len()].max(1);
        let end = (offset + configured).min(payload.len());
        output
            .write_all(&payload[offset..end])
            .expect("write fragmented fake PCM");
        output.flush().expect("flush fragmented fake PCM");
        offset = end;
        fragment += 1;
    }
}

fn close_stdout() {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn close(fd: i32) -> i32;
        }
        // SAFETY: this process is the fixture and intentionally closes its own
        // stdout descriptor to model FFmpeg closing the PCM pipe before exit.
        let _ = unsafe { close(1) };
    }
    #[cfg(target_os = "windows")]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetStdHandle(kind: u32) -> *mut std::ffi::c_void;
            fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        }
        const STD_OUTPUT_HANDLE: u32 = -11_i32 as u32;
        // SAFETY: the handle comes directly from GetStdHandle and belongs to
        // this short-lived fixture process.
        let _ = unsafe { CloseHandle(GetStdHandle(STD_OUTPUT_HANDLE)) };
    }
}

fn main() {
    let executable = std::env::current_exe().expect("current fake executable");
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "--fixture-probe") {
        return;
    }
    #[cfg(target_os = "windows")]
    if args.iter().any(|arg| arg == "--job-owner-probe") {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build job-owner probe runtime");
        let _capture = runtime
            .block_on(app_lib::audio::directshow::DirectShowCapture::start(
                executable.clone(),
                "Lifecycle Microphone".to_string(),
                |_| {},
                |_| {},
            ))
            .expect("start job-contained fake capture");
        append(&sidecar_path(&executable, "events"), "owner-ready");
        // Deliberately bypass Rust Drop. Windows closes this process's Job
        // handle and KILL_ON_JOB_CLOSE must terminate the assigned FFmpeg.
        std::process::exit(0);
    }
    let config: Config = serde_json::from_slice(
        &fs::read(sidecar_path(&executable, "json")).expect("read fake config"),
    )
    .expect("parse fake config");
    let enumeration = args.iter().any(|arg| arg == "-list_devices");
    append(
        &sidecar_path(&executable, "pids"),
        &format!(
            "{},{}",
            if enumeration {
                "enumeration"
            } else {
                "capture"
            },
            std::process::id()
        ),
    );
    append(
        &sidecar_path(&executable, "args"),
        &args
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\u{1f}"),
    );

    if enumeration {
        thread::sleep(Duration::from_millis(config.enumeration_delay_ms));
        io::stderr()
            .write_all(config.enumeration_stderr.as_bytes())
            .expect("write fake enumeration");
        std::process::exit(config.exit_code);
    }

    if config.stderr_bytes > 0 {
        let count = config.stderr_bytes;
        thread::spawn(move || {
            let mut stderr = io::stderr().lock();
            let private = b"error PRIVATE_DEVICE_MODEL_ID PRIVATE_TRANSCRIPT_PAYLOAD\n";
            let mut written = 0;
            while written < count {
                let size = (count - written).min(private.len());
                if stderr.write_all(&private[..size]).is_err() {
                    return;
                }
                written += size;
            }
            let _ = stderr.flush();
        });
    }

    thread::sleep(Duration::from_millis(config.initial_delay_ms));
    let payload = match config.invalid_pcm.as_deref() {
        Some("nonfinite") => f32::NAN.to_le_bytes().to_vec(),
        Some("nonfinite_after_initial") => {
            let mut payload = bytes(config.initial_value, config.initial_samples);
            payload.extend_from_slice(&f32::NAN.to_le_bytes());
            payload
        }
        Some("truncated") => vec![0x01, 0x02, 0x03],
        Some(other) => panic!("unsupported invalid_pcm value {other}"),
        None => bytes(config.initial_value, config.initial_samples),
    };
    write_fragmented(io::stdout().lock(), &payload, &config.byte_fragments);
    if config.close_stdout_after_initial {
        close_stdout();
    }

    if config.wait_for_quit {
        let mut byte = [0_u8; 1];
        loop {
            match io::stdin().read(&mut byte) {
                Ok(0) => {
                    if config.ignore_quit {
                        thread::sleep(Duration::from_secs(60));
                    }
                    break;
                }
                Ok(_) if byte[0] == b'q' => {
                    append(&sidecar_path(&executable, "events"), "quit");
                    if config.ignore_quit {
                        thread::sleep(Duration::from_secs(60));
                    }
                    thread::sleep(Duration::from_millis(config.exit_delay_after_quit_ms));
                    break;
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    }

    if config.tail_samples > 0 {
        let tail = bytes(config.tail_value, config.tail_samples);
        io::stdout().write_all(&tail).expect("write fake PCM tail");
        io::stdout().flush().expect("flush fake PCM tail");
    }
    std::process::exit(config.exit_code);
}
