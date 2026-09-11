//! Validated, atomic import of a completed GigaSTT result.

use super::types::{GigasttResult, WordInfo};
use serde_json::json;
use sqlx::{Acquire, SqlitePool};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const TIMESTAMP_EPSILON_SECONDS: f64 = 0.001;
const ALIGNMENT_LOOKAHEAD_TOKENS: usize = 32;

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedRow {
    pub id: String,
    pub meeting_id: String,
    pub transcript: String,
    pub timestamp: String,
    pub audio_start_time: f64,
    pub audio_end_time: f64,
    pub duration: f64,
    pub speaker: Option<String>,
}

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("invalid GigaSTT result field: {field}")]
    InvalidResult { field: &'static str },
    #[error("meeting does not exist")]
    MeetingNotFound,
    #[error("transcript import cancelled")]
    Cancelled,
    #[error("result metadata could not be encoded")]
    Metadata(#[from] serde_json::Error),
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
}

/// Validate a complete result and map it to the existing `transcripts` schema.
pub fn map_result(
    meeting_id: &str,
    result: &GigasttResult,
) -> Result<Vec<ImportedRow>, ImportError> {
    validate_result(meeting_id, result)?;

    let encoded_result = serde_json::to_vec(result)?;
    let meeting_namespace = Uuid::new_v5(&Uuid::NAMESPACE_OID, meeting_id.as_bytes());
    let result_namespace = Uuid::new_v5(&meeting_namespace, &encoded_result);
    let timestamp = chrono::Utc::now().to_rfc3339();
    let rows = match result
        .segments
        .as_ref()
        .filter(|segments| !segments.is_empty())
    {
        Some(segments) => formatted_segments(segments, &result.text)
            .into_iter()
            .enumerate()
            .map(|(index, segment)| ImportedRow {
                id: stable_row_id(&result_namespace, index),
                meeting_id: meeting_id.to_string(),
                transcript: segment.text,
                timestamp: timestamp.clone(),
                audio_start_time: segment.start,
                audio_end_time: segment.end,
                duration: segment.end - segment.start,
                speaker: segment.speaker.map(speaker_label),
            })
            .collect(),
        None => {
            let first_word = result.words.first().expect("validated non-empty words");
            let last_word = result.words.last().expect("validated non-empty words");
            vec![ImportedRow {
                id: stable_row_id(&result_namespace, 0),
                meeting_id: meeting_id.to_string(),
                transcript: result.text.trim().to_string(),
                timestamp,
                audio_start_time: first_word.start,
                audio_end_time: last_word.end,
                duration: last_word.end - first_word.start,
                speaker: unanimous_speaker(&result.words).map(speaker_label),
            }]
        }
    };

    Ok(rows)
}

#[derive(Debug)]
struct CanonicalToken {
    start: usize,
    end: usize,
    normalized: String,
}

#[derive(Debug)]
struct RawToken {
    segment_index: usize,
    normalized: String,
}

#[derive(Debug)]
struct FormattedSegment {
    start: f64,
    end: f64,
    text: String,
    speaker: Option<u32>,
}

/// Project GigaSTT's authoritative formatted text back onto its raw timed
/// segments. GigaSTT deliberately leaves `words` and `segments[].text` raw
/// after applying ITN and RuPunct to the top-level `text`, so segment text
/// cannot be used directly for display.
///
/// The aligner is monotonic and bounded: it searches at most a small fixed
/// token window for the next unchanged anchor. Replacement spans (including
/// number-word -> digit ITN) stay with one timed segment when possible. If a
/// replacement crosses segment/speaker boundaries, those rows are merged and
/// the speaker is kept only when every merged segment agrees. This preserves
/// all canonical text without guessing who said an ambiguous rewritten span.
fn formatted_segments(
    segments: &[super::types::Segment],
    canonical_text: &str,
) -> Vec<FormattedSegment> {
    let canonical_text = canonical_text.trim();
    let canonical_tokens = tokenize_canonical(canonical_text);
    let raw_tokens = tokenize_segments(segments);

    if canonical_tokens.is_empty() || raw_tokens.is_empty() {
        return vec![fallback_segment(segments, canonical_text)];
    }

    let mut components = SegmentComponents::new(segments.len());
    let mut owners = vec![None; canonical_tokens.len()];
    let mut raw_index = 0;
    let mut canonical_index = 0;

    while raw_index < raw_tokens.len() && canonical_index < canonical_tokens.len() {
        if tokens_match(
            &raw_tokens[raw_index].normalized,
            &canonical_tokens[canonical_index].normalized,
        ) {
            owners[canonical_index] = Some(raw_tokens[raw_index].segment_index);
            raw_index += 1;
            canonical_index += 1;
            continue;
        }

        if let Some((next_raw, next_canonical)) =
            find_next_anchor(&raw_tokens, &canonical_tokens, raw_index, canonical_index)
        {
            assign_replacement(
                &raw_tokens,
                &canonical_tokens,
                raw_index..next_raw,
                canonical_index..next_canonical,
                &mut owners,
                &mut components,
            );
            owners[next_canonical] = Some(raw_tokens[next_raw].segment_index);
            raw_index = next_raw + 1;
            canonical_index = next_canonical + 1;
        } else {
            // A fixed lookahead keeps long recordings linear. If no reliable
            // anchor is nearby, conservatively treat the remaining suffix as
            // one replacement instead of making distant/repeated-word guesses.
            assign_replacement(
                &raw_tokens,
                &canonical_tokens,
                raw_index..raw_tokens.len(),
                canonical_index..canonical_tokens.len(),
                &mut owners,
                &mut components,
            );
            raw_index = raw_tokens.len();
            canonical_index = canonical_tokens.len();
        }
    }

    assign_replacement(
        &raw_tokens,
        &canonical_tokens,
        raw_index..raw_tokens.len(),
        canonical_index..canonical_tokens.len(),
        &mut owners,
        &mut components,
    );

    if owners.iter().any(Option::is_none) {
        return vec![fallback_segment(segments, canonical_text)];
    }

    attach_segments_without_text(segments.len(), &owners, &mut components);
    build_formatted_segments(
        segments,
        canonical_text,
        &canonical_tokens,
        &owners,
        &mut components,
    )
    .unwrap_or_else(|| vec![fallback_segment(segments, canonical_text)])
}

fn tokenize_canonical(text: &str) -> Vec<CanonicalToken> {
    let mut tokens = Vec::new();
    let mut token_start = None;

    for (index, character) in text.char_indices() {
        if character.is_whitespace() {
            if let Some(start) = token_start.take() {
                tokens.push(CanonicalToken {
                    start,
                    end: index,
                    normalized: normalize_token(&text[start..index]),
                });
            }
        } else if token_start.is_none() {
            token_start = Some(index);
        }
    }
    if let Some(start) = token_start {
        tokens.push(CanonicalToken {
            start,
            end: text.len(),
            normalized: normalize_token(&text[start..]),
        });
    }

    tokens
}

fn tokenize_segments(segments: &[super::types::Segment]) -> Vec<RawToken> {
    segments
        .iter()
        .enumerate()
        .flat_map(|(segment_index, segment)| {
            segment.words.iter().flat_map(move |word| {
                word.word.split_whitespace().filter_map(move |token| {
                    let normalized = normalize_token(token);
                    (!normalized.is_empty()).then_some(RawToken {
                        segment_index,
                        normalized,
                    })
                })
            })
        })
        .collect()
}

fn normalize_token(token: &str) -> String {
    token
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn tokens_match(raw: &str, canonical: &str) -> bool {
    !raw.is_empty() && raw == canonical
}

fn find_next_anchor(
    raw: &[RawToken],
    canonical: &[CanonicalToken],
    raw_start: usize,
    canonical_start: usize,
) -> Option<(usize, usize)> {
    let raw_end = raw
        .len()
        .min(raw_start.saturating_add(ALIGNMENT_LOOKAHEAD_TOKENS + 1));
    let canonical_end = canonical
        .len()
        .min(canonical_start.saturating_add(ALIGNMENT_LOOKAHEAD_TOKENS + 1));
    let raw_window = raw_end - raw_start;
    let canonical_window = canonical_end - canonical_start;
    let lcp_stride = canonical_window + 1;
    let mut matching_run_lengths = vec![0_usize; (raw_window + 1) * lcp_stride];

    // Compute every candidate's matching forward context once. This keeps the
    // bounded search quadratic in the fixed window instead of comparing a
    // suffix separately for every repeated-token candidate.
    let mut raw_offset = raw_window;
    while raw_offset > 0 {
        raw_offset -= 1;
        let mut canonical_offset = canonical_window;
        while canonical_offset > 0 {
            canonical_offset -= 1;
            if tokens_match(
                &raw[raw_start + raw_offset].normalized,
                &canonical[canonical_start + canonical_offset].normalized,
            ) {
                matching_run_lengths[raw_offset * lcp_stride + canonical_offset] =
                    1 + matching_run_lengths[(raw_offset + 1) * lcp_stride + canonical_offset + 1];
            }
        }
    }

    // Candidates with the same canonical position and equal forward context
    // but different timed segments are indistinguishable. Exclude all of them
    // so a later reliable anchor can bound a conservative merged replacement.
    let support_stride = raw_window.min(canonical_window) + 1;
    let mut group_segment = vec![None; canonical_window * support_stride];
    let mut ambiguous_group = vec![false; canonical_window * support_stride];
    for (raw_offset, raw_token) in raw[raw_start..raw_end].iter().enumerate() {
        for canonical_offset in 0..canonical_window {
            let support = matching_run_lengths[raw_offset * lcp_stride + canonical_offset];
            if support == 0 {
                continue;
            }
            let group_index = canonical_offset * support_stride + support;
            match group_segment[group_index] {
                None => group_segment[group_index] = Some(raw_token.segment_index),
                Some(segment) if segment != raw_token.segment_index => {
                    ambiguous_group[group_index] = true;
                }
                Some(_) => {}
            }
        }
    }

    let mut best: Option<AnchorCandidate> = None;

    for (raw_index, raw_token) in raw.iter().enumerate().take(raw_end).skip(raw_start) {
        for (canonical_index, canonical_token) in canonical
            .iter()
            .enumerate()
            .take(canonical_end)
            .skip(canonical_start)
        {
            let raw_offset = raw_index - raw_start;
            let canonical_offset = canonical_index - canonical_start;
            let support = matching_run_lengths[raw_offset * lcp_stride + canonical_offset];
            if support == 0 {
                continue;
            }
            let group_index = canonical_offset * support_stride + support;
            if ambiguous_group[group_index] {
                continue;
            }
            debug_assert!(tokens_match(
                &raw_token.normalized,
                &canonical_token.normalized
            ));
            let rank = AnchorRank {
                total_distance: raw_offset + canonical_offset,
                max_distance: raw_offset.max(canonical_offset),
                raw_distance: raw_offset,
                canonical_distance: canonical_offset,
            };
            let candidate = AnchorCandidate {
                support,
                rank,
                anchor: (raw_index, canonical_index),
            };
            if best.as_ref().is_none_or(|current| {
                candidate.support > current.support
                    || (candidate.support == current.support && candidate.rank < current.rank)
            }) {
                best = Some(candidate);
            }
        }
    }

    best.map(|candidate| candidate.anchor)
}

#[derive(Debug, Clone, Copy)]
struct AnchorCandidate {
    support: usize,
    rank: AnchorRank,
    anchor: (usize, usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct AnchorRank {
    total_distance: usize,
    max_distance: usize,
    raw_distance: usize,
    canonical_distance: usize,
}

fn assign_replacement(
    raw: &[RawToken],
    canonical: &[CanonicalToken],
    raw_range: std::ops::Range<usize>,
    canonical_range: std::ops::Range<usize>,
    owners: &mut [Option<usize>],
    components: &mut SegmentComponents,
) {
    if raw_range.is_empty() && canonical_range.is_empty() {
        return;
    }

    let mut raw_owners = raw[raw_range.clone()]
        .iter()
        .map(|token| token.segment_index);
    let first_raw_owner = raw_owners.next();
    if !canonical_range.is_empty() {
        if let Some(owner) = first_raw_owner {
            for other in raw_owners {
                components.union(owner, other);
            }
            for canonical_index in canonical_range {
                owners[canonical_index] = Some(owner);
            }
            return;
        }

        let previous_owner = raw_range
            .start
            .checked_sub(1)
            .and_then(|index| raw.get(index))
            .map(|token| token.segment_index);
        let next_owner = raw.get(raw_range.end).map(|token| token.segment_index);
        let contains_spoken_text = canonical[canonical_range.clone()]
            .iter()
            .any(|token| !token.normalized.is_empty());
        let owner = match (previous_owner, next_owner) {
            (Some(previous), Some(next)) if previous != next && contains_spoken_text => {
                components.union(previous, next);
                previous
            }
            (Some(previous), _) => previous,
            (None, Some(next)) => next,
            (None, None) => return,
        };
        for canonical_index in canonical_range {
            owners[canonical_index] = Some(owner);
        }
    }
}

fn attach_segments_without_text(
    segment_count: usize,
    owners: &[Option<usize>],
    components: &mut SegmentComponents,
) {
    let mut has_text = vec![false; segment_count];
    for owner in owners.iter().flatten() {
        let root = components.find(*owner);
        has_text[root] = true;
    }

    let mut nearest_left = vec![None; segment_count];
    let mut last_text_segment = None;
    for (segment_index, nearest) in nearest_left.iter_mut().enumerate() {
        let root = components.find(segment_index);
        if has_text[root] {
            last_text_segment = Some(segment_index);
        }
        *nearest = last_text_segment;
    }

    let mut nearest_right = vec![None; segment_count];
    let mut next_text_segment = None;
    for (segment_index, nearest) in nearest_right.iter_mut().enumerate().rev() {
        let root = components.find(segment_index);
        if has_text[root] {
            next_text_segment = Some(segment_index);
        }
        *nearest = next_text_segment;
    }

    for segment_index in 0..segment_count {
        let root = components.find(segment_index);
        if !has_text[root] {
            if let Some(neighbour) = nearest_left[segment_index].or(nearest_right[segment_index]) {
                components.union(segment_index, neighbour);
            }
        }
    }
}

fn build_formatted_segments(
    segments: &[super::types::Segment],
    canonical_text: &str,
    canonical_tokens: &[CanonicalToken],
    owners: &[Option<usize>],
    components: &mut SegmentComponents,
) -> Option<Vec<FormattedSegment>> {
    let mut groups: Vec<(usize, usize, usize)> = Vec::new();
    let mut seen_roots = vec![false; segments.len()];
    for (token_index, owner) in owners.iter().copied().enumerate() {
        let root = components.find(owner?);
        if groups
            .last()
            .is_some_and(|(last_root, _, _)| *last_root == root)
        {
            groups.last_mut().expect("checked non-empty").2 = token_index + 1;
        } else if seen_roots[root] {
            return None;
        } else {
            seen_roots[root] = true;
            groups.push((root, token_index, token_index + 1));
        }
    }

    let mut first_segments = vec![usize::MAX; segments.len()];
    let mut last_segments = vec![0; segments.len()];
    let mut segment_counts = vec![0; segments.len()];
    for segment_index in 0..segments.len() {
        let root = components.find(segment_index);
        first_segments[root] = first_segments[root].min(segment_index);
        last_segments[root] = segment_index;
        segment_counts[root] += 1;
    }

    let mut formatted = Vec::with_capacity(groups.len());
    for (root, token_start, token_end) in groups {
        let first_segment = first_segments[root];
        let last_segment = last_segments[root];
        if first_segment == usize::MAX || segment_counts[root] != last_segment - first_segment + 1 {
            return None;
        }
        let text_start = canonical_tokens[token_start].start;
        let text_end = canonical_tokens[token_end - 1].end;
        let text = canonical_text[text_start..text_end].trim().to_string();
        if text.is_empty() {
            return None;
        }
        formatted.push(FormattedSegment {
            start: segments[first_segment].start,
            end: segments[last_segment].end,
            text,
            speaker: unanimous_segment_speaker(&segments[first_segment..=last_segment]),
        });
    }

    (!formatted.is_empty()).then_some(formatted)
}

fn fallback_segment(segments: &[super::types::Segment], text: &str) -> FormattedSegment {
    let first = segments.first().expect("validated non-empty segments");
    let last = segments.last().expect("validated non-empty segments");
    FormattedSegment {
        start: first.start,
        end: last.end,
        text: text.trim().to_string(),
        speaker: unanimous_segment_speaker(segments),
    }
}

fn unanimous_segment_speaker(segments: &[super::types::Segment]) -> Option<u32> {
    let first = resolved_segment_speaker(segments.first()?);
    let speaker = first?;
    segments
        .iter()
        .all(|segment| resolved_segment_speaker(segment) == Some(speaker))
        .then_some(speaker)
}

fn resolved_segment_speaker(segment: &super::types::Segment) -> Option<u32> {
    segment
        .speaker
        .or_else(|| unanimous_speaker(&segment.words))
}

#[derive(Debug)]
struct SegmentComponents {
    parent: Vec<usize>,
}

impl SegmentComponents {
    fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
        }
    }

    fn find(&mut self, mut index: usize) -> usize {
        while self.parent[index] != index {
            let parent = self.parent[index];
            self.parent[index] = self.parent[parent];
            index = self.parent[index];
        }
        index
    }

    fn union(&mut self, left: usize, right: usize) {
        let left_root = self.find(left);
        let right_root = self.find(right);
        if left_root != right_root {
            self.parent[right_root] = left_root;
        }
    }
}

/// Replace the visible transcript and its authority metadata in one transaction.
pub async fn replace_transcript(
    pool: &SqlitePool,
    meeting_id: &str,
    result: &GigasttResult,
    cancel: &CancellationToken,
) -> Result<usize, ImportError> {
    let rows = map_result(meeting_id, result)?;
    let result_metadata = serde_json::to_string(&json!({
        "text": &result.text,
        "duration_seconds": result.duration,
        "confidence": result.confidence,
        "words": &result.words,
        "segments": &result.segments,
    }))?;

    check_cancelled(cancel)?;
    let mut connection = pool.acquire().await?;
    let mut transaction = connection.begin().await?;
    let meeting_exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM meetings WHERE id = ?")
        .bind(meeting_id)
        .fetch_optional(&mut *transaction)
        .await?;
    if meeting_exists.is_none() {
        return Err(ImportError::MeetingNotFound);
    }

    // This is the first destructive statement.  Everything below remains in
    // the same transaction, and every early return therefore restores the old
    // draft/final rows and metadata.
    check_cancelled(cancel)?;
    sqlx::query("DELETE FROM transcripts WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    for row in &rows {
        check_cancelled(cancel)?;
        sqlx::query(
            "INSERT INTO transcripts
             (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration, speaker)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.id)
        .bind(&row.meeting_id)
        .bind(&row.transcript)
        .bind(&row.timestamp)
        .bind(row.audio_start_time)
        .bind(row.audio_end_time)
        .bind(row.duration)
        .bind(&row.speaker)
        .execute(&mut *transaction)
        .await?;
    }

    check_cancelled(cancel)?;
    sqlx::query(
        "INSERT INTO meeting_transcript_metadata
         (meeting_id, provenance, result_metadata, updated_at)
         VALUES (?, 'final_gigastt', ?, ?)
         ON CONFLICT(meeting_id) DO UPDATE SET
             provenance = excluded.provenance,
             result_metadata = excluded.result_metadata,
             updated_at = excluded.updated_at",
    )
    .bind(meeting_id)
    .bind(result_metadata)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&mut *transaction)
    .await?;

    check_cancelled(cancel)?;
    transaction.commit().await?;
    Ok(rows.len())
}

fn validate_result(meeting_id: &str, result: &GigasttResult) -> Result<(), ImportError> {
    if meeting_id.trim().is_empty() {
        return invalid("meeting_id");
    }
    if result.text.trim().is_empty() {
        return invalid("text");
    }
    if !result.duration.is_finite() || result.duration <= 0.0 {
        return invalid("duration");
    }
    validate_confidence(result.confidence, "confidence")?;
    validate_words(&result.words, result.duration, None)?;

    if let Some(segments) = result
        .segments
        .as_ref()
        .filter(|segments| !segments.is_empty())
    {
        let mut previous_start = 0.0;
        for (index, segment) in segments.iter().enumerate() {
            validate_interval(
                segment.start,
                segment.end,
                result.duration,
                "segment_timing",
            )?;
            if index > 0 && segment.start < previous_start {
                return invalid("segment_order");
            }
            if segment.text.trim().is_empty() {
                return invalid("segment_text");
            }
            if segment.words.is_empty() {
                return invalid("segment_words");
            }
            validate_words(
                &segment.words,
                result.duration,
                Some((segment.start, segment.end)),
            )?;
            previous_start = segment.start;
        }
    } else if result.words.is_empty() {
        return invalid("timings");
    }

    Ok(())
}

fn validate_words(
    words: &[WordInfo],
    result_duration: f64,
    segment_bounds: Option<(f64, f64)>,
) -> Result<(), ImportError> {
    let mut previous_start = 0.0;
    for (index, word) in words.iter().enumerate() {
        if word.word.trim().is_empty() {
            return invalid("word_text");
        }
        validate_interval(word.start, word.end, result_duration, "word_timing")?;
        if index > 0 && word.start < previous_start {
            return invalid("word_order");
        }
        if let Some((segment_start, segment_end)) = segment_bounds {
            if word.start + TIMESTAMP_EPSILON_SECONDS < segment_start
                || word.end > segment_end + TIMESTAMP_EPSILON_SECONDS
            {
                return invalid("word_segment_timing");
            }
        }
        validate_confidence(word.confidence, "word_confidence")?;
        previous_start = word.start;
    }
    Ok(())
}

fn validate_interval(
    start: f64,
    end: f64,
    result_duration: f64,
    field: &'static str,
) -> Result<(), ImportError> {
    if !start.is_finite()
        || !end.is_finite()
        || start < 0.0
        || end < start
        || end > result_duration + TIMESTAMP_EPSILON_SECONDS
    {
        return invalid(field);
    }
    Ok(())
}

fn validate_confidence(confidence: Option<f32>, field: &'static str) -> Result<(), ImportError> {
    if confidence.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
        return invalid(field);
    }
    Ok(())
}

fn invalid<T>(field: &'static str) -> Result<T, ImportError> {
    Err(ImportError::InvalidResult { field })
}

fn stable_row_id(result_namespace: &Uuid, index: usize) -> String {
    format!(
        "transcript-{}",
        Uuid::new_v5(result_namespace, &(index as u64).to_be_bytes())
    )
}

fn unanimous_speaker(words: &[WordInfo]) -> Option<u32> {
    let speaker = words.first()?.speaker?;
    words
        .iter()
        .all(|word| word.speaker == Some(speaker))
        .then_some(speaker)
}

fn speaker_label(speaker: u32) -> String {
    format!("speaker_{speaker}")
}

fn check_cancelled(cancel: &CancellationToken) -> Result<(), ImportError> {
    if cancel.is_cancelled() {
        Err(ImportError::Cancelled)
    } else {
        Ok(())
    }
}
