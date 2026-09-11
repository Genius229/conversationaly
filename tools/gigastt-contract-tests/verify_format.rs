//! Verify the desktop's real row mapper against a live native jobs result.
//! The input remains runner-local; never print transcript text into build logs.
use app_lib::{importer::map_result, types::GigasttResult};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("expected result JSON path")?;
    let result: GigasttResult = serde_json::from_slice(&std::fs::read(path)?)?;
    let rows = map_result("native-format-smoke", &result)?;
    let visible = rows
        .iter()
        .map(|row| row.transcript.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    if visible != result.text.trim() {
        return Err("visible transcript differs from the canonical native result".into());
    }
    if !visible.chars().any(|ch| ch.is_uppercase())
        || !visible
            .chars()
            .any(|ch| matches!(ch, '.' | ',' | '?' | '!' | ':' | ';'))
    {
        return Err("native fixture did not retain punctuation and casing".into());
    }
    println!(
        "CANONICAL PROJECTION PASS rows={} punctuation=true casing=true",
        rows.len()
    );
    Ok(())
}
