//! Small PCM16 WAV writer for normalized post-recording input.
use std::{
    io::{self, BufWriter, Write},
    path::Path,
};

/// Writes only normalized temporary audio. Validate before touching the target,
/// then atomically replace it from a same-directory temporary file.
pub fn write_pcm16_wav(path: &Path, samples: &[f32]) -> io::Result<()> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "invalid normalized audio");
    if samples.is_empty() || samples.iter().any(|v| !v.is_finite()) {
        return Err(invalid());
    }
    let data_len = samples
        .len()
        .checked_mul(2)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n <= u32::MAX - 36)
        .ok_or_else(invalid)?;
    let parent = path.parent().ok_or_else(invalid)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut out = BufWriter::new(temp.as_file_mut());
        out.write_all(b"RIFF")?;
        out.write_all(&(data_len + 36).to_le_bytes())?;
        out.write_all(b"WAVEfmt ")?;
        out.write_all(&16_u32.to_le_bytes())?;
        out.write_all(&1_u16.to_le_bytes())?; // PCM
        out.write_all(&1_u16.to_le_bytes())?; // mono
        out.write_all(&16000_u32.to_le_bytes())?;
        out.write_all(&32000_u32.to_le_bytes())?; // byte rate
        out.write_all(&2_u16.to_le_bytes())?; // block align
        out.write_all(&16_u16.to_le_bytes())?;
        out.write_all(b"data")?;
        out.write_all(&data_len.to_le_bytes())?;
        for sample in samples {
            let pcm = (sample.clamp(-1.0, 1.0) * 32768.0)
                .round()
                .clamp(-32768.0, 32767.0) as i16;
            out.write_all(&pcm.to_le_bytes())?;
        }
        out.flush()?;
    }
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}
