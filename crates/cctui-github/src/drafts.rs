//! GH-VIEW-4: the native review-draft store + CRUD.
//!
//! A reviewer adds inline comments **instantly** (no GitHub round-trip) into a
//! local draft, anchored on the GH-VIEW-2 `(path, side, line[, start_line])`
//! coordinates. The draft is refined and then published as one batched review
//! by GH-VIEW-5. This module owns the `github.review_drafts` /
//! `github.review_draft_comments` rows: the row↔proto mapping (pure, unit
//! tested) and the DB-backed CRUD the routes call.
//!
//! Author model: a human reviewer (`user`, the owning user — one open draft per
//! user+pull) or a review agent (`agent`, a cctui session). No credential or
//! token is ever represented or logged here.

use cctui_proto::github::{DiffSide, DraftAuthorKind, DraftStatus, ReviewVerdict};

// ---- on-wire token <-> column-string mapping (pure) -----------------------

/// Column string for a [`DiffSide`] (`old`/`new`) — matches the
/// `review_draft_comments_side_chk` constraint and the proto `snake_case`.
#[must_use]
pub fn side_str(s: DiffSide) -> &'static str {
    match s {
        DiffSide::Old => "old",
        DiffSide::New => "new",
    }
}

/// Parse a stored side string back to [`DiffSide`]. Unknown values fall back to
/// `New` (the head side) rather than failing a read — the column is CHECK-
/// constrained on write, so this only guards against a future schema drift.
#[must_use]
pub fn side_from_str(s: &str) -> DiffSide {
    match s {
        "old" => DiffSide::Old,
        _ => DiffSide::New,
    }
}

/// Column string for a [`ReviewVerdict`].
#[must_use]
pub fn verdict_str(v: ReviewVerdict) -> &'static str {
    match v {
        ReviewVerdict::Comment => "comment",
        ReviewVerdict::Approve => "approve",
        ReviewVerdict::RequestChanges => "request_changes",
    }
}

/// Parse a stored verdict string. Unknown values fall back to `Comment`.
#[must_use]
pub fn verdict_from_str(s: &str) -> ReviewVerdict {
    match s {
        "approve" => ReviewVerdict::Approve,
        "request_changes" => ReviewVerdict::RequestChanges,
        _ => ReviewVerdict::Comment,
    }
}

/// Parse a stored author-kind string. Unknown values fall back to `User`.
#[must_use]
pub fn author_kind_from_str(s: &str) -> DraftAuthorKind {
    match s {
        "agent" => DraftAuthorKind::Agent,
        _ => DraftAuthorKind::User,
    }
}

/// Parse a stored draft-status string. Unknown values fall back to `Draft`.
#[must_use]
pub fn status_from_str(s: &str) -> DraftStatus {
    match s {
        "published" => DraftStatus::Published,
        _ => DraftStatus::Draft,
    }
}

// ---- DB layer -------------------------------------------------------------

mod db {
    use super::{
        author_kind_from_str, side_from_str, side_str, status_from_str, verdict_from_str,
        verdict_str,
    };
    use cctui_proto::github::{
        CreateDraftComment, DraftCommentInfo, ReviewDraftInfo, ReviewVerdict,
    };
    use sqlx::{PgPool, Row};
    use uuid::Uuid;

    /// Outcome of a draft CRUD operation the route maps to an HTTP status.
    #[derive(Debug, PartialEq, Eq)]
    pub enum DraftError {
        /// No draft/comment matched the id (and scope).
        NotFound,
        /// A database error (already logged at the call site).
        Db,
    }

    fn comment_from_row(row: &sqlx::postgres::PgRow) -> Result<DraftCommentInfo, sqlx::Error> {
        let start: Option<i64> = row.try_get("start_line")?;
        Ok(DraftCommentInfo {
            id: row.try_get("id")?,
            draft_id: row.try_get("draft_id")?,
            path: row.try_get("path")?,
            side: side_from_str(row.try_get::<String, _>("side")?.as_str()),
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            line: row.try_get::<i64, _>("line")? as u32,
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            start_line: start.map(|v| v as u32),
            body: row.try_get("body")?,
            github_comment_id: row.try_get("github_comment_id")?,
            in_reply_to: row.try_get("in_reply_to")?,
            created_at: row.try_get::<chrono::DateTime<chrono::Utc>, _>("created_at")?.to_rfc3339(),
            updated_at: row.try_get::<chrono::DateTime<chrono::Utc>, _>("updated_at")?.to_rfc3339(),
        })
    }

    /// Load a draft (header + comments) by id, scoped to a connector + PR ref.
    async fn load_draft(
        pool: &PgPool,
        connector_id: Uuid,
        repo: &str,
        number: i64,
        draft_id: Uuid,
    ) -> Result<Option<ReviewDraftInfo>, sqlx::Error> {
        let Some(row) = sqlx::query(
            "SELECT id, connector_id, repo, pull_number, author_kind, author_user_id, \
                    author_session_id, verdict, status, created_at, updated_at \
             FROM github.review_drafts \
             WHERE id = $1 AND connector_id = $2 AND repo = $3 AND pull_number = $4",
        )
        .bind(draft_id)
        .bind(connector_id)
        .bind(repo)
        .bind(number)
        .fetch_optional(pool)
        .await?
        else {
            return Ok(None);
        };

        let comments = sqlx::query(
            "SELECT id, draft_id, path, side, line, start_line, body, github_comment_id, \
                    in_reply_to, created_at, updated_at \
             FROM github.review_draft_comments WHERE draft_id = $1 ORDER BY created_at, id",
        )
        .bind(draft_id)
        .fetch_all(pool)
        .await?
        .iter()
        .map(comment_from_row)
        .collect::<Result<Vec<_>, _>>()?;

        Ok(Some(ReviewDraftInfo {
            id: row.try_get("id")?,
            connector_id: row.try_get("connector_id")?,
            repo: row.try_get("repo")?,
            number: row.try_get("pull_number")?,
            author_kind: author_kind_from_str(row.try_get::<String, _>("author_kind")?.as_str()),
            author_user_id: row.try_get("author_user_id")?,
            author_session_id: row.try_get("author_session_id")?,
            verdict: verdict_from_str(row.try_get::<String, _>("verdict")?.as_str()),
            status: status_from_str(row.try_get::<String, _>("status")?.as_str()),
            created_at: row.try_get::<chrono::DateTime<chrono::Utc>, _>("created_at")?.to_rfc3339(),
            updated_at: row.try_get::<chrono::DateTime<chrono::Utc>, _>("updated_at")?.to_rfc3339(),
            comments,
        }))
    }

    /// Open (or reuse) the caller's open draft for a PR. One open draft per
    /// user+pull: a second open returns the existing row instead of conflicting.
    pub async fn open_user_draft(
        pool: &PgPool,
        connector_id: Uuid,
        repo: &str,
        number: i64,
        user_id: Uuid,
        verdict: ReviewVerdict,
    ) -> Result<ReviewDraftInfo, DraftError> {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO github.review_drafts \
                (connector_id, repo, pull_number, author_kind, author_user_id, verdict) \
             VALUES ($1, $2, $3, 'user', $4, $5) \
             ON CONFLICT (connector_id, repo, pull_number, author_user_id) \
                 WHERE status = 'draft' AND author_kind = 'user' \
             DO UPDATE SET updated_at = now() \
             RETURNING id",
        )
        .bind(connector_id)
        .bind(repo)
        .bind(number)
        .bind(user_id)
        .bind(verdict_str(verdict))
        .fetch_one(pool)
        .await
        .map_err(|_| DraftError::Db)?;

        load_draft(pool, connector_id, repo, number, id)
            .await
            .map_err(|_| DraftError::Db)?
            .ok_or(DraftError::Db)
    }

    /// List the caller's open + published drafts for a PR (header + comments).
    /// A user sees only their own drafts (plus any agent drafts on the PR).
    pub async fn list_drafts(
        pool: &PgPool,
        connector_id: Uuid,
        repo: &str,
        number: i64,
        user_id: Option<Uuid>,
    ) -> Result<Vec<ReviewDraftInfo>, DraftError> {
        // A user (Some) sees their own + agent drafts; admin (None) sees all.
        let ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM github.review_drafts \
             WHERE connector_id = $1 AND repo = $2 AND pull_number = $3 \
               AND ($4::uuid IS NULL OR author_user_id = $4 OR author_kind = 'agent') \
             ORDER BY created_at",
        )
        .bind(connector_id)
        .bind(repo)
        .bind(number)
        .bind(user_id)
        .fetch_all(pool)
        .await
        .map_err(|_| DraftError::Db)?;

        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(d) = load_draft(pool, connector_id, repo, number, id)
                .await
                .map_err(|_| DraftError::Db)?
            {
                out.push(d);
            }
        }
        Ok(out)
    }

    /// Change a draft's verdict.
    pub async fn update_verdict(
        pool: &PgPool,
        connector_id: Uuid,
        repo: &str,
        number: i64,
        draft_id: Uuid,
        verdict: ReviewVerdict,
    ) -> Result<ReviewDraftInfo, DraftError> {
        let res = sqlx::query(
            "UPDATE github.review_drafts SET verdict = $1, updated_at = now() \
             WHERE id = $2 AND connector_id = $3 AND repo = $4 AND pull_number = $5 \
               AND status = 'draft'",
        )
        .bind(verdict_str(verdict))
        .bind(draft_id)
        .bind(connector_id)
        .bind(repo)
        .bind(number)
        .execute(pool)
        .await
        .map_err(|_| DraftError::Db)?;
        if res.rows_affected() == 0 {
            return Err(DraftError::NotFound);
        }
        load_draft(pool, connector_id, repo, number, draft_id)
            .await
            .map_err(|_| DraftError::Db)?
            .ok_or(DraftError::NotFound)
    }

    /// Delete a draft (and, via cascade, its comments).
    pub async fn delete_draft(
        pool: &PgPool,
        connector_id: Uuid,
        repo: &str,
        number: i64,
        draft_id: Uuid,
    ) -> Result<(), DraftError> {
        let res = sqlx::query(
            "DELETE FROM github.review_drafts \
             WHERE id = $1 AND connector_id = $2 AND repo = $3 AND pull_number = $4",
        )
        .bind(draft_id)
        .bind(connector_id)
        .bind(repo)
        .bind(number)
        .execute(pool)
        .await
        .map_err(|_| DraftError::Db)?;
        if res.rows_affected() == 0 {
            return Err(DraftError::NotFound);
        }
        Ok(())
    }

    /// Add one inline comment to an open draft (anchored on the selection).
    /// Returns the refreshed draft so the UI re-renders inline atomically.
    pub async fn add_comment(
        pool: &PgPool,
        connector_id: Uuid,
        repo: &str,
        number: i64,
        draft_id: Uuid,
        c: &CreateDraftComment,
    ) -> Result<ReviewDraftInfo, DraftError> {
        // Guard: the comment's draft must exist + be in scope + still open.
        let open: Option<bool> = sqlx::query_scalar(
            "SELECT status = 'draft' FROM github.review_drafts \
             WHERE id = $1 AND connector_id = $2 AND repo = $3 AND pull_number = $4",
        )
        .bind(draft_id)
        .bind(connector_id)
        .bind(repo)
        .bind(number)
        .fetch_optional(pool)
        .await
        .map_err(|_| DraftError::Db)?;
        match open {
            None => return Err(DraftError::NotFound),
            Some(false) => return Err(DraftError::NotFound),
            Some(true) => {}
        }

        sqlx::query(
            "INSERT INTO github.review_draft_comments \
                (draft_id, path, side, line, start_line, body, in_reply_to) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(draft_id)
        .bind(&c.path)
        .bind(side_str(c.side))
        .bind(i64::from(c.line))
        .bind(c.start_line.map(i64::from))
        .bind(&c.body)
        .bind(c.in_reply_to)
        .execute(pool)
        .await
        .map_err(|_| DraftError::Db)?;

        // Bump the draft's updated_at so the inbox reflects activity.
        let _ = sqlx::query("UPDATE github.review_drafts SET updated_at = now() WHERE id = $1")
            .bind(draft_id)
            .execute(pool)
            .await;

        load_draft(pool, connector_id, repo, number, draft_id)
            .await
            .map_err(|_| DraftError::Db)?
            .ok_or(DraftError::NotFound)
    }

    /// Edit a draft comment's body in place (the anchor is immutable).
    pub async fn update_comment(
        pool: &PgPool,
        connector_id: Uuid,
        repo: &str,
        number: i64,
        draft_id: Uuid,
        comment_id: Uuid,
        body: &str,
    ) -> Result<ReviewDraftInfo, DraftError> {
        let res = sqlx::query(
            "UPDATE github.review_draft_comments c SET body = $1, updated_at = now() \
             FROM github.review_drafts d \
             WHERE c.id = $2 AND c.draft_id = $3 AND d.id = c.draft_id \
               AND d.connector_id = $4 AND d.repo = $5 AND d.pull_number = $6",
        )
        .bind(body)
        .bind(comment_id)
        .bind(draft_id)
        .bind(connector_id)
        .bind(repo)
        .bind(number)
        .execute(pool)
        .await
        .map_err(|_| DraftError::Db)?;
        if res.rows_affected() == 0 {
            return Err(DraftError::NotFound);
        }
        load_draft(pool, connector_id, repo, number, draft_id)
            .await
            .map_err(|_| DraftError::Db)?
            .ok_or(DraftError::NotFound)
    }

    /// Delete one draft comment. Returns the refreshed draft.
    pub async fn delete_comment(
        pool: &PgPool,
        connector_id: Uuid,
        repo: &str,
        number: i64,
        draft_id: Uuid,
        comment_id: Uuid,
    ) -> Result<ReviewDraftInfo, DraftError> {
        let res = sqlx::query(
            "DELETE FROM github.review_draft_comments c \
             USING github.review_drafts d \
             WHERE c.id = $1 AND c.draft_id = $2 AND d.id = c.draft_id \
               AND d.connector_id = $3 AND d.repo = $4 AND d.pull_number = $5",
        )
        .bind(comment_id)
        .bind(draft_id)
        .bind(connector_id)
        .bind(repo)
        .bind(number)
        .execute(pool)
        .await
        .map_err(|_| DraftError::Db)?;
        if res.rows_affected() == 0 {
            return Err(DraftError::NotFound);
        }
        load_draft(pool, connector_id, repo, number, draft_id)
            .await
            .map_err(|_| DraftError::Db)?
            .ok_or(DraftError::NotFound)
    }
}

pub use db::{
    DraftError, add_comment, delete_comment, delete_draft, list_drafts, open_user_draft,
    update_comment, update_verdict,
};

#[cfg(test)]
mod tests {
    use super::*;
    use cctui_proto::github::{DraftAuthorKind, DraftStatus};

    #[test]
    fn side_round_trips_through_column_strings() {
        assert_eq!(side_str(DiffSide::Old), "old");
        assert_eq!(side_str(DiffSide::New), "new");
        assert_eq!(side_from_str("old"), DiffSide::Old);
        assert_eq!(side_from_str("new"), DiffSide::New);
        // Maps to GitHub's LEFT/RIGHT exactly via the proto helper.
        assert_eq!(side_from_str("old").github_token(), "LEFT");
        assert_eq!(side_from_str("new").github_token(), "RIGHT");
    }

    #[test]
    fn side_from_unknown_falls_back_to_new() {
        assert_eq!(side_from_str("bogus"), DiffSide::New);
    }

    #[test]
    fn verdict_round_trips() {
        for v in [ReviewVerdict::Comment, ReviewVerdict::Approve, ReviewVerdict::RequestChanges] {
            assert_eq!(verdict_from_str(verdict_str(v)), v);
        }
    }

    #[test]
    fn verdict_default_string_matches_migration_default() {
        // The migration defaults verdict to 'comment'; the proto round-trip must
        // agree so a row created without an explicit verdict reads back as Comment.
        assert_eq!(verdict_str(ReviewVerdict::Comment), "comment");
        assert_eq!(verdict_from_str("comment"), ReviewVerdict::Comment);
    }

    #[test]
    fn author_kind_and_status_parse() {
        assert_eq!(author_kind_from_str("user"), DraftAuthorKind::User);
        assert_eq!(author_kind_from_str("agent"), DraftAuthorKind::Agent);
        assert_eq!(author_kind_from_str("???"), DraftAuthorKind::User);
        assert_eq!(status_from_str("draft"), DraftStatus::Draft);
        assert_eq!(status_from_str("published"), DraftStatus::Published);
        assert_eq!(status_from_str("???"), DraftStatus::Draft);
    }
}
