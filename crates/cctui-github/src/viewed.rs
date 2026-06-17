//! GH-VIEW-6: blob-keyed "reviewed" marks (docs/github-integration.md §6.2).
//!
//! A reviewer marks a file reviewed **keyed to its blob SHA** (`DiffFile.blob_sha`,
//! surfaced by GH-VIEW-1). The mark therefore records "I reviewed THIS content of
//! THIS path". When the diff reloads after a push, a file is still considered
//! reviewed only while its *current* blob SHA equals the stored mark — so a push
//! re-flags ONLY the files that actually changed (their blob SHA rotated), while
//! unchanged files keep their matching SHA and stay reviewed.
//!
//! The re-flag is a **pure comparison** the webui does at render time against the
//! marks this module returns; we never bulk-rewrite marks on a push. A mark is
//! per `(user, connector, repo, pull_number, path)` — re-marking the same path
//! updates its blob SHA + timestamp in place (idempotent). No credential or
//! token is represented or logged here.

use cctui_proto::github::ViewedMarkInfo;

/// The pure re-flag predicate the read path applies.
///
/// Whether a stored mark's `blob_sha` still matches the file's current blob SHA.
/// A file with no current blob SHA (e.g. removed, or a SHA GitHub omitted) can
/// never match, so it re-flags as unreviewed rather than appearing stale.
#[must_use]
pub fn is_still_reviewed(marked_blob_sha: &str, current_blob_sha: Option<&str>) -> bool {
    current_blob_sha == Some(marked_blob_sha)
}

// ---- DB layer -------------------------------------------------------------

mod db {
    use super::ViewedMarkInfo;
    use sqlx::{PgPool, Row};
    use uuid::Uuid;

    /// Outcome of a viewed-mark operation the route maps to an HTTP status.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ViewedError {
        /// No mark matched the path (unmark of an unmarked file).
        NotFound,
        /// A database error (already logged at the call site).
        Db,
    }

    fn mark_from_row(row: &sqlx::postgres::PgRow) -> Result<ViewedMarkInfo, sqlx::Error> {
        Ok(ViewedMarkInfo {
            path: row.try_get("path")?,
            blob_sha: row.try_get("blob_sha")?,
            marked_at: row.try_get::<chrono::DateTime<chrono::Utc>, _>("marked_at")?.to_rfc3339(),
        })
    }

    /// Mark a file reviewed keyed to `blob_sha`.
    ///
    /// Idempotent: re-marking the same path updates the stored blob SHA +
    /// timestamp in place (one row per file), so marking the same content twice
    /// is a no-op beyond bumping `marked_at`, and marking again after a push
    /// records the new blob SHA.
    pub async fn mark(
        pool: &PgPool,
        user_id: Uuid,
        connector_id: Uuid,
        repo: &str,
        number: i64,
        path: &str,
        blob_sha: &str,
    ) -> Result<(), ViewedError> {
        sqlx::query(
            "INSERT INTO github.viewed_marks \
                (user_id, connector_id, repo, pull_number, path, blob_sha) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (user_id, connector_id, repo, pull_number, path) \
             DO UPDATE SET blob_sha = EXCLUDED.blob_sha, marked_at = now()",
        )
        .bind(user_id)
        .bind(connector_id)
        .bind(repo)
        .bind(number)
        .bind(path)
        .bind(blob_sha)
        .execute(pool)
        .await
        .map_err(|_| ViewedError::Db)?;
        Ok(())
    }

    /// Remove a file's reviewed mark (the path is the identity — the blob SHA is
    /// irrelevant to an unmark). `NotFound` if the file was not marked.
    pub async fn unmark(
        pool: &PgPool,
        user_id: Uuid,
        connector_id: Uuid,
        repo: &str,
        number: i64,
        path: &str,
    ) -> Result<(), ViewedError> {
        let res = sqlx::query(
            "DELETE FROM github.viewed_marks \
             WHERE user_id = $1 AND connector_id = $2 AND repo = $3 \
               AND pull_number = $4 AND path = $5",
        )
        .bind(user_id)
        .bind(connector_id)
        .bind(repo)
        .bind(number)
        .bind(path)
        .execute(pool)
        .await
        .map_err(|_| ViewedError::Db)?;
        if res.rows_affected() == 0 {
            return Err(ViewedError::NotFound);
        }
        Ok(())
    }

    /// List the caller's marks for a PR. The webui pairs each mark's `blob_sha`
    /// with the current diff: a file stays reviewed only while its current blob
    /// SHA still matches (see [`super::is_still_reviewed`]).
    pub async fn list(
        pool: &PgPool,
        user_id: Uuid,
        connector_id: Uuid,
        repo: &str,
        number: i64,
    ) -> Result<Vec<ViewedMarkInfo>, ViewedError> {
        sqlx::query(
            "SELECT path, blob_sha, marked_at FROM github.viewed_marks \
             WHERE user_id = $1 AND connector_id = $2 AND repo = $3 AND pull_number = $4 \
             ORDER BY path",
        )
        .bind(user_id)
        .bind(connector_id)
        .bind(repo)
        .bind(number)
        .fetch_all(pool)
        .await
        .map_err(|_| ViewedError::Db)?
        .iter()
        .map(mark_from_row)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ViewedError::Db)
    }
}

pub use db::{ViewedError, list, mark, unmark};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_blob_sha_stays_reviewed() {
        // Same content (blob SHA unchanged) on reload → still reviewed.
        assert!(is_still_reviewed("abc123", Some("abc123")));
    }

    #[test]
    fn changed_blob_sha_re_flags() {
        // A push rotated this file's blob SHA → no longer reviewed.
        assert!(!is_still_reviewed("abc123", Some("def456")));
    }

    #[test]
    fn missing_current_blob_sha_re_flags() {
        // File dropped from the diff / SHA omitted → cannot be matched.
        assert!(!is_still_reviewed("abc123", None));
    }

    #[test]
    fn re_flag_is_per_file_not_whole_pr() {
        // Two files marked at one push; a later push changes only one of them.
        // The unchanged file stays reviewed; only the changed one re-flags.
        let marks = [("src/a.rs", "sha_a"), ("src/b.rs", "sha_b")];
        // After the push: a.rs unchanged, b.rs changed.
        let current = |p: &str| match p {
            "src/a.rs" => Some("sha_a"),  // unchanged
            "src/b.rs" => Some("sha_b2"), // changed
            _ => None,
        };
        let reviewed: Vec<_> = marks
            .iter()
            .filter(|(p, marked)| is_still_reviewed(marked, current(p)))
            .map(|(p, _)| *p)
            .collect();
        assert_eq!(reviewed, vec!["src/a.rs"]);
    }
}
