use base64::Engine;

use super::validate::{self, PLACEHOLDER};

/// Longest token-shaped run a placeholder is taken to stand for.
const MAX_MASKED_RUN: usize = 4096;
const PREFIX_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intact {
    pub hash: String,
    pub size: u64,
    pub media_type: Option<String>,
}

#[must_use]
pub fn segments(text: &str) -> Option<Vec<&str>> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(PLACEHOLDER) {
        out.push(&rest[..start]);
        let end = rest[start..].find(']')?;
        rest = &rest[start + end + 1..];
    }
    if out.is_empty() {
        return None;
    }
    out.push(rest);
    Some(out)
}

/// Whether `intact` is `segs` with every placeholder standing for a non-empty run.
#[must_use]
pub fn fits(intact: &str, segs: &[&str]) -> bool {
    let [first, middle @ .., last] = segs else {
        return false;
    };
    if !intact.starts_with(first) || !intact.ends_with(last) {
        return false;
    }
    let Some(end) = intact.len().checked_sub(last.len()) else {
        return false;
    };
    let mut at = first.len();
    for seg in middle {
        let Some(found) = intact.get(at + 1..end).and_then(|s| s.find(seg)) else {
            return false;
        };
        at += 1 + found + seg.len();
    }
    at < end
}

fn byte_window(segs: &[&str]) -> (i64, i64) {
    let kept: usize = segs.iter().map(|s| s.len()).sum();
    let runs = segs.len() - 1;
    let lo = ((kept + runs) * 3 / 4).saturating_sub(3);
    let hi = (kept + runs * MAX_MASKED_RUN) * 3 / 4 + 3;
    (i64::try_from(lo).unwrap_or(i64::MAX), i64::try_from(hi).unwrap_or(i64::MAX))
}

const CANDIDATES_SQL: &str = "WITH owner AS (SELECT user_id FROM sessions WHERE id = $1) \
     SELECT b.hash, b.bytes FROM daemon_blobs b \
     WHERE b.byte_len BETWEEN $2 AND $3 AND b.media_type LIKE 'image/%' \
       AND substring(b.bytes FROM 1 FOR $4) = $5 \
       AND (EXISTS (SELECT 1 FROM session_blob_links l JOIN sessions s ON s.id = l.session_id \
                    WHERE l.hash = b.hash AND s.user_id = (SELECT user_id FROM owner)) \
         OR EXISTS (SELECT 1 FROM session_attachments a JOIN sessions s ON s.id = a.session_id \
                    WHERE a.hash = b.hash AND s.user_id = (SELECT user_id FROM owner)))";

/// The one image blob of the session's owner that `text` is a redacted copy
/// of. Several matches are as good as none.
pub async fn find_intact(
    conn: &mut sqlx::PgConnection,
    session_id: &str,
    text: &str,
) -> Result<Option<Intact>, sqlx::Error> {
    let Some(segs) = segments(text) else {
        return Ok(None);
    };
    let head = &segs[0][..(segs[0].len() / 4 * 4).min(PREFIX_BYTES / 3 * 4)];
    let Ok(prefix) = base64::engine::general_purpose::STANDARD.decode(head) else {
        return Ok(None);
    };
    let (lo, hi) = byte_window(&segs);
    let rows: Vec<(String, Vec<u8>)> = sqlx::query_as(CANDIDATES_SQL)
        .bind(session_id)
        .bind(lo)
        .bind(hi)
        .bind(i32::try_from(prefix.len()).unwrap_or(i32::MAX))
        .bind(&prefix)
        .fetch_all(conn)
        .await?;
    let mut found = None;
    for (hash, bytes) in rows {
        if !fits(&base64::engine::general_purpose::STANDARD.encode(&bytes), &segs) {
            continue;
        }
        let Ok(valid) = validate::check_bytes(None, bytes) else { continue };
        if valid.media_type.is_none() {
            continue;
        }
        if found.is_some() {
            return Ok(None);
        }
        found = Some(Intact { hash, size: valid.bytes.len() as u64, media_type: valid.media_type });
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_split_on_each_placeholder() {
        let text = "ab[REDACTED:square_token]cd[REDACTED:discord_bot_token:9f2a]ef";
        assert_eq!(segments(text), Some(vec!["ab", "cd", "ef"]));
        assert_eq!(segments("[REDACTED:x]"), Some(vec!["", ""]));
        assert_eq!(segments("abcdef"), None, "nothing masked");
        assert_eq!(segments("ab[REDACTED:x"), None, "unterminated");
    }

    #[test]
    fn fits_needs_every_kept_run_in_order_around_non_empty_gaps() {
        let segs = ["ab", "cd", "ef"];
        assert!(fits("abXcdYYef", &segs));
        assert!(fits("abcdXcdYef", &segs), "the first cd may sit inside a masked run");
        assert!(!fits("abcdYef", &segs), "a masked run is never empty");
        assert!(!fits("abXcdef", &segs));
        assert!(!fits("zbXcdYef", &segs));
        assert!(!fits("abXcdYeg", &segs));
        assert!(!fits("abXefYcd", &segs), "out of order");
        assert!(fits("abXef", &["ab", "ef"]));
        assert!(!fits("abef", &["ab", "ef"]));
        assert!(!fits("a", &["a", "a"]));
    }

    #[test]
    fn the_size_window_covers_the_masked_runs() {
        let (lo, hi) = byte_window(&["A".repeat(400).as_str(), "B".repeat(400).as_str()]);
        let widest = i64::try_from((800 + MAX_MASKED_RUN) * 3 / 4).unwrap();
        assert!(lo <= 601 && hi >= widest, "{lo}..{hi}");
    }
}
