use std::ffi::OsString;

pub const ENUMERATION_BYTE_LIMIT: usize = 128 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeviceSelectionError {
    #[error("DirectShow device listing exceeded {limit} bytes")]
    OutputTooLarge { limit: usize },
    #[error("DirectShow device listing was not UTF-8")]
    InvalidUtf8,
    #[error("DirectShow device listing was incomplete or malformed")]
    Malformed,
    #[error("Selected microphone was not found in DirectShow")]
    Missing,
    #[error("Selected microphone is ambiguous in DirectShow ({matches} matches)")]
    Ambiguous { matches: usize },
}

pub fn enumeration_args() -> Vec<OsString> {
    [
        "-hide_banner",
        "-nostats",
        "-nostdin",
        "-list_devices",
        "true",
        "-f",
        "dshow",
        "-i",
        "dummy",
    ]
    .into_iter()
    .map(OsString::from)
    .collect()
}

pub fn capture_args(token: &str) -> Vec<OsString> {
    let mut args: Vec<_> = [
        "-hide_banner",
        "-nostats",
        "-loglevel",
        "error",
        "-f",
        "dshow",
        "-i",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(format!("audio={token}").into());
    args.extend(
        [
            "-map",
            "0:a:0",
            "-ac",
            "1",
            "-ar",
            "48000",
            "-c:a",
            "pcm_f32le",
            "-f",
            "f32le",
            "pipe:1",
        ]
        .into_iter()
        .map(OsString::from),
    );
    args
}

fn valid_field(value: &str) -> bool {
    !value.trim().is_empty() && !value.chars().any(char::is_control)
}

/// FFmpeg 8.x prints UTF-8, unescaped quoted friendly names followed by a
/// separate Alternative name record. Parse from the right so embedded quotes
/// do not change the field boundary. Never fall back to the first friendly name.
pub fn select_device(stderr: &[u8], exact_name: &str) -> Result<String, DeviceSelectionError> {
    if stderr.len() > ENUMERATION_BYTE_LIMIT {
        return Err(DeviceSelectionError::OutputTooLarge {
            limit: ENUMERATION_BYTE_LIMIT,
        });
    }
    if !valid_field(exact_name) {
        return Err(DeviceSelectionError::Malformed);
    }
    let text = std::str::from_utf8(stderr).map_err(|_| DeviceSelectionError::InvalidUtf8)?;
    let mut pending: Option<(&str, &str, bool)> = None;
    let mut matches = Vec::new();
    for line in text.lines() {
        let body = line
            .strip_prefix("[dshow @ ")
            .and_then(|line| line.split_once(']'))
            .map(|(context, body)| (context, body.trim_start()));
        let Some((context, body)) = body else {
            if pending.is_some() {
                return Err(DeviceSelectionError::Malformed);
            }
            continue;
        };
        if let Some(value) = body.strip_prefix("Alternative name \"") {
            let moniker = value
                .strip_suffix('"')
                .ok_or(DeviceSelectionError::Malformed)?;
            if !valid_field(moniker) || moniker.contains(':') {
                return Err(DeviceSelectionError::Malformed);
            }
            let (device_context, name, audio) =
                pending.take().ok_or(DeviceSelectionError::Malformed)?;
            if device_context != context {
                return Err(DeviceSelectionError::Malformed);
            }
            if audio && name == exact_name {
                matches.push(moniker.to_owned());
            }
        } else if let Some(record) = body.strip_prefix('"') {
            if pending.is_some() {
                return Err(DeviceSelectionError::Malformed);
            }
            let (name, types) = record
                .rsplit_once("\" (")
                .ok_or(DeviceSelectionError::Malformed)?;
            let types = types
                .strip_suffix(')')
                .ok_or(DeviceSelectionError::Malformed)?;
            if !valid_field(name) || types.is_empty() {
                return Err(DeviceSelectionError::Malformed);
            }
            let mut audio = false;
            for media in types.split(',').map(str::trim) {
                match media {
                    "audio" => audio = true,
                    "video" | "unknown" => {}
                    "none" if types == "none" => {}
                    _ => return Err(DeviceSelectionError::Malformed),
                }
            }
            pending = Some((context, name, audio));
        } else if pending.is_some() {
            return Err(DeviceSelectionError::Malformed);
        }
    }
    if pending.is_some() {
        return Err(DeviceSelectionError::Malformed);
    }
    match matches.len() {
        0 => Err(DeviceSelectionError::Missing),
        1 => Ok(matches.remove(0)),
        matches => Err(DeviceSelectionError::Ambiguous { matches }),
    }
}
