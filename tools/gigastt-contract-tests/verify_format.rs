//! Verify the desktop's real row mapper against a live native jobs result.
//! The input remains runner-local; never print transcript text into build logs.
use app_lib::{importer::map_result, types::GigasttResult};

fn valid_format(text: &str, require_uppercase: bool) -> bool {
    // ITN can replace the only capitalized word with a leading number.
    // A second native pass without ITN explicitly requires uppercase letters.
    (text.chars().any(|ch| ch.is_uppercase())
        || (!require_uppercase && text.starts_with(|ch: char| ch.is_ascii_digit())))
        && text
            .chars()
            .any(|ch| matches!(ch, '.' | ',' | '?' | '!' | ':' | ';'))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let path = args.next().ok_or("expected result JSON path")?;
    let require_uppercase = match args.next() {
        None => false,
        Some(arg) if arg == "--require-uppercase" => true,
        Some(_) => return Err("unknown verification option".into()),
    };
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
    if !valid_format(&visible, require_uppercase) {
        return Err("native fixture did not retain punctuation and casing".into());
    }
    println!(
        "CANONICAL PROJECTION PASS rows={} punctuation=true uppercase={} numeric_start={} require_uppercase={}",
        rows.len(),
        visible.chars().any(|ch| ch.is_uppercase()),
        visible.starts_with(|ch: char| ch.is_ascii_digit()),
        require_uppercase
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_leading_itn_does_not_require_a_capital_letter() {
        assert!(valid_format("60000 тенге, сколько будет стоить?", false));
        assert!(!valid_format("60000 тенге, сколько будет стоить?", true));
    }

    #[test]
    fn alphabetic_fixture_must_retain_case_and_punctuation() {
        assert!(valid_format("Шестьдесят тысяч тенге?", true));
        assert!(!valid_format("шестьдесят тысяч тенге?", false));
        assert!(!valid_format("Шестьдесят тысяч тенге", true));
        assert!(!valid_format("", false));
    }
}
