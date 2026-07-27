//! GH-CONN-2: the `POST /api/v1/triggers/github` webhook ingress.
//!
//! GitHub posts a JSON event with two headers we care about:
//! - `X-GitHub-Event` — the event name (`pull_request`, `push`, …); routes to a
//!   parser.
//! - `X-Hub-Signature-256` — `sha256=<hex>`, an HMAC-SHA256 of the **raw** body
//!   keyed by the connector's webhook secret. We verify it before parsing.
//!
//! The webhook carries no connector id, so we verify the signature against each
//! connector that has a webhook secret and accept the first whose HMAC matches
//! in constant time. A body that no connector signs is rejected `401`.
//!
//! Parsed objects feed the GH-CONN-3 [`crate::store`] upsert functions, which
//! broadcast a `ServerEvent` on success. Unknown/unhandled event types are a
//! `204` no-op. We never log the signature, the secret, or the raw payload.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use cctui_proto::github::{CheckUpsert, PullUpsert, ReviewCommentUpsert, ReviewUpsert};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use uuid::Uuid;

use crate::{GithubState, classifier_feed, crypto, store};

type HmacSha256 = Hmac<Sha256>;

/// `POST /api/v1/triggers/github` — verify the signature, route by event type,
/// and upsert. Returns `401` on a bad/absent signature, `202` on an accepted
/// event, `204` for an unhandled event type, and `400` on a malformed payload.
pub async fn webhook(
    State(state): State<GithubState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let Some(signature) = header(&headers, "x-hub-signature-256") else {
        return StatusCode::UNAUTHORIZED;
    };
    let event = header(&headers, "x-github-event").unwrap_or_default().to_string();

    // Find a connector whose stored webhook secret signs this exact body.
    let Some(connector_id) = match_connector(&state, &body, signature).await else {
        return StatusCode::UNAUTHORIZED;
    };

    match dispatch(&state, connector_id, &event, &body).await {
        Ok(Handled::Accepted) => StatusCode::ACCEPTED,
        Ok(Handled::Ignored) => StatusCode::NO_CONTENT,
        Err(DispatchError::BadPayload) => StatusCode::BAD_REQUEST,
        Err(DispatchError::Db) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// Find the connector whose webhook secret produces `signature` over `body`.
///
/// We decrypt each configured secret, recompute the HMAC, and compare in
/// constant time (the `hmac` crate's `verify_slice`). The first match wins.
/// Returns `None` if no connector matches — the caller answers `401`.
async fn match_connector(state: &GithubState, body: &[u8], signature: &str) -> Option<Uuid> {
    // `sha256=<hex>`; reject anything else without leaking which part failed.
    let hex_sig = signature.strip_prefix("sha256=")?;
    let expected = hex::decode(hex_sig).ok()?;

    let key = crypto::vault_key();
    let rows: Vec<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT id, encrypted_webhook_secret FROM github.connectors \
         WHERE encrypted_webhook_secret IS NOT NULL",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    for (id, enc) in rows {
        let Some(enc) = enc else { continue };
        let Some(secret) = crypto::decrypt(&enc, &key) else { continue };
        let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else { continue };
        mac.update(body);
        if mac.verify_slice(&expected).is_ok() {
            return Some(id);
        }
    }
    None
}

enum Handled {
    Accepted,
    Ignored,
}

enum DispatchError {
    BadPayload,
    Db,
}

impl From<sqlx::Error> for DispatchError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!("github webhook store error: {e}");
        Self::Db
    }
}

/// Route a verified event to its parser + store upsert(s).
async fn dispatch(
    state: &GithubState,
    connector_id: Uuid,
    event: &str,
    body: &[u8],
) -> Result<Handled, DispatchError> {
    let v: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| DispatchError::BadPayload)?;

    match event {
        "pull_request" | "pull_request_review_thread" => {
            let pull = parse_pull(&v).ok_or(DispatchError::BadPayload)?;
            store::upsert_pull(&state.pool, &state.events, connector_id, &pull).await?;
            // GH-CLS-1: enrich the classifier cache for the session that opened
            // this PR.
            classifier_feed::refresh(
                &state.pool,
                &state.pr_cache,
                connector_id,
                &pull.repo,
                pull.number,
            )
            .await;
            Ok(Handled::Accepted)
        }
        "pull_request_review" => {
            // Carries both the PR snapshot and the submitted review.
            if let Some(pull) = parse_pull(&v) {
                store::upsert_pull(&state.pool, &state.events, connector_id, &pull).await?;
            }
            let review = parse_review(&v).ok_or(DispatchError::BadPayload)?;
            store::upsert_review(&state.pool, &state.events, connector_id, &review).await?;
            classifier_feed::refresh(
                &state.pool,
                &state.pr_cache,
                connector_id,
                &review.repo,
                review.pull_number,
            )
            .await;
            Ok(Handled::Accepted)
        }
        "pull_request_review_comment" | "issue_comment" => {
            let comment = parse_review_comment(&v).ok_or(DispatchError::BadPayload)?;
            store::upsert_review_comment(&state.pool, &state.events, connector_id, &comment)
                .await?;
            Ok(Handled::Accepted)
        }
        "check_suite" | "check_run" => {
            let check = parse_check(event, &v).ok_or(DispatchError::BadPayload)?;
            store::upsert_check(&state.pool, &state.events, connector_id, &check).await?;
            refresh_pulls_at_sha(state, connector_id, &check.repo, &check.head_sha).await;
            Ok(Handled::Accepted)
        }
        "status" => {
            let check = parse_status(&v).ok_or(DispatchError::BadPayload)?;
            store::upsert_check(&state.pool, &state.events, connector_id, &check).await?;
            refresh_pulls_at_sha(state, connector_id, &check.repo, &check.head_sha).await;
            Ok(Handled::Accepted)
        }
        "push" => {
            // A push has no PR object on its own; the head SHA moved, so any
            // tracked checks for that SHA are stale. We have no row to upsert
            // here without a PR lookup, so treat push as an accepted no-op: the
            // reconcile poll (GH-CONN-4) repairs PR head SHAs. Accepting (not
            // 204) keeps GitHub's delivery log green for a configured event.
            Ok(Handled::Accepted)
        }
        _ => Ok(Handled::Ignored),
    }
}

/// GH-CLS-1: a check/status event keys on a head SHA, not a PR number, so map
/// the SHA back to the open PR(s) on it and refresh each one's classifier
/// status. Best-effort: a query error refreshes nothing (the cache keeps its
/// prior view) rather than failing the webhook.
async fn refresh_pulls_at_sha(state: &GithubState, connector_id: Uuid, repo: &str, head_sha: &str) {
    let numbers: Vec<(i64,)> = sqlx::query_as(
        "SELECT number FROM github.pulls \
         WHERE connector_id = $1 AND repo = $2 AND head_sha = $3",
    )
    .bind(connector_id)
    .bind(repo)
    .bind(head_sha)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();
    for (number,) in numbers {
        classifier_feed::refresh(&state.pool, &state.pr_cache, connector_id, repo, number).await;
    }
}

// ---- payload parsers -------------------------------------------------------
//
// GitHub payloads are large and partly optional; we pull only the fields the
// store needs and treat a missing required field as a bad payload. `repo()`
// resolves the `owner/name` slug shared by every event shape.

fn repo(v: &serde_json::Value) -> Option<String> {
    v.get("repository")?.get("full_name")?.as_str().map(str::to_string)
}

fn parse_pull(v: &serde_json::Value) -> Option<PullUpsert> {
    let pr = v.get("pull_request")?;
    Some(PullUpsert {
        node_id: pr.get("node_id")?.as_str()?.to_string(),
        repo: repo(v)?,
        number: pr.get("number")?.as_i64()?,
        title: pr.get("title").and_then(|t| t.as_str()).unwrap_or_default().to_string(),
        state: pr.get("state").and_then(|s| s.as_str()).unwrap_or("open").to_string(),
        merged: pr.get("merged").and_then(serde_json::Value::as_bool).unwrap_or(false),
        draft: pr.get("draft").and_then(serde_json::Value::as_bool).unwrap_or(false),
        mergeable_state: pr.get("mergeable_state").and_then(|s| s.as_str()).map(str::to_string),
        author: pr
            .get("user")
            .and_then(|u| u.get("login"))
            .and_then(|l| l.as_str())
            .unwrap_or_default()
            .to_string(),
        head_sha: pr
            .get("head")
            .and_then(|h| h.get("sha"))
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string(),
        base_ref: pr
            .get("base")
            .and_then(|b| b.get("ref"))
            .and_then(|r| r.as_str())
            .unwrap_or_default()
            .to_string(),
        head_ref: pr
            .get("head")
            .and_then(|h| h.get("ref"))
            .and_then(|r| r.as_str())
            .unwrap_or_default()
            .to_string(),
        gh_created_at: pr
            .get("created_at")
            .and_then(|t| t.as_str())
            .unwrap_or("1970-01-01T00:00:00Z")
            .to_string(),
        gh_updated_at: pr
            .get("updated_at")
            .and_then(|t| t.as_str())
            .unwrap_or("1970-01-01T00:00:00Z")
            .to_string(),
    })
}

fn parse_review(v: &serde_json::Value) -> Option<ReviewUpsert> {
    let review = v.get("review")?;
    Some(ReviewUpsert {
        repo: repo(v)?,
        pull_number: v.get("pull_request")?.get("number")?.as_i64()?,
        review_id: review.get("id")?.as_i64()?,
        reviewer: review
            .get("user")
            .and_then(|u| u.get("login"))
            .and_then(|l| l.as_str())
            .unwrap_or_default()
            .to_string(),
        state: review.get("state").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
        body: review.get("body").and_then(|b| b.as_str()).map(str::to_string),
        commit_id: review.get("commit_id").and_then(|c| c.as_str()).map(str::to_string),
        submitted_at: review.get("submitted_at").and_then(|t| t.as_str()).map(str::to_string),
    })
}

fn parse_review_comment(v: &serde_json::Value) -> Option<ReviewCommentUpsert> {
    let comment = v.get("comment")?;
    // `pull_request_review_comment` carries `pull_request`; `issue_comment`
    // carries `issue` (with a `pull_request` sub-object only when it is a PR).
    let pull_number = v
        .get("pull_request")
        .and_then(|p| p.get("number"))
        .and_then(serde_json::Value::as_i64)
        .or_else(|| v.get("issue")?.get("number")?.as_i64())?;
    Some(ReviewCommentUpsert {
        repo: repo(v)?,
        pull_number,
        comment_id: comment.get("id")?.as_i64()?,
        thread_node_id: comment.get("node_id").and_then(|n| n.as_str()).map(str::to_string),
        author: comment
            .get("user")
            .and_then(|u| u.get("login"))
            .and_then(|l| l.as_str())
            .unwrap_or_default()
            .to_string(),
        body: comment.get("body").and_then(|b| b.as_str()).unwrap_or_default().to_string(),
        path: comment.get("path").and_then(|p| p.as_str()).map(str::to_string),
        side: comment.get("side").and_then(|s| s.as_str()).map(str::to_string),
        line: comment.get("line").and_then(serde_json::Value::as_i64),
        gh_created_at: comment
            .get("created_at")
            .and_then(|t| t.as_str())
            .unwrap_or("1970-01-01T00:00:00Z")
            .to_string(),
        gh_updated_at: comment
            .get("updated_at")
            .and_then(|t| t.as_str())
            .unwrap_or("1970-01-01T00:00:00Z")
            .to_string(),
    })
}

fn parse_check(event: &str, v: &serde_json::Value) -> Option<CheckUpsert> {
    let obj = v.get(event)?; // `check_suite` or `check_run`
    let head_sha = obj.get("head_sha")?.as_str()?.to_string();
    Some(CheckUpsert {
        repo: repo(v)?,
        head_sha,
        external_id: obj.get("id")?.as_i64()?.to_string(),
        name: obj
            .get("name")
            .or_else(|| obj.get("app").and_then(|a| a.get("name")))
            .and_then(|n| n.as_str())
            .unwrap_or(event)
            .to_string(),
        status: obj.get("status").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
        conclusion: obj.get("conclusion").and_then(|c| c.as_str()).map(str::to_string),
        details_url: obj.get("details_url").and_then(|u| u.as_str()).map(str::to_string),
    })
}

fn parse_status(v: &serde_json::Value) -> Option<CheckUpsert> {
    // The commit-status event: a flat object keyed by context + sha.
    let head_sha = v.get("sha")?.as_str()?.to_string();
    let context = v.get("context")?.as_str()?.to_string();
    Some(CheckUpsert {
        repo: repo(v)?,
        head_sha,
        // Statuses have no numeric id; the context is stable per commit.
        external_id: format!("status:{context}"),
        name: context,
        // Map the status `state` onto the check status/conclusion shape.
        status: "completed".to_string(),
        conclusion: v.get("state").and_then(|s| s.as_str()).map(str::to_string),
        details_url: v.get("target_url").and_then(|u| u.as_str()).map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sign(secret: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    #[test]
    fn signature_verifies_for_correct_secret() {
        let secret = "topsecret";
        let body = br#"{"hello":"world"}"#;
        let sig = sign(secret, body);
        let hex_sig = sig.strip_prefix("sha256=").unwrap();
        let expected = hex::decode(hex_sig).unwrap();
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        assert!(mac.verify_slice(&expected).is_ok());
    }

    #[test]
    fn signature_rejects_wrong_secret() {
        let body = br#"{"hello":"world"}"#;
        let sig = sign("topsecret", body);
        let hex_sig = sig.strip_prefix("sha256=").unwrap();
        let expected = hex::decode(hex_sig).unwrap();
        let mut mac = HmacSha256::new_from_slice(b"wrong").unwrap();
        mac.update(body);
        assert!(mac.verify_slice(&expected).is_err());
    }

    #[test]
    fn signature_rejects_tampered_body() {
        let secret = "topsecret";
        let sig = sign(secret, br#"{"hello":"world"}"#);
        let hex_sig = sig.strip_prefix("sha256=").unwrap();
        let expected = hex::decode(hex_sig).unwrap();
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(br#"{"hello":"tampered"}"#);
        assert!(mac.verify_slice(&expected).is_err());
    }

    #[test]
    fn parse_pull_extracts_core_fields() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{
                "action": "opened",
                "repository": { "full_name": "o/r" },
                "pull_request": {
                    "node_id": "PR_node", "number": 42, "title": "t",
                    "state": "open", "merged": false, "draft": true,
                    "mergeable_state": "clean",
                    "user": { "login": "me" },
                    "head": { "sha": "abc", "ref": "feat" },
                    "base": { "ref": "main" },
                    "created_at": "2026-06-17T00:00:00Z",
                    "updated_at": "2026-06-17T01:00:00Z"
                }
            }"#,
        )
        .unwrap();
        let p = parse_pull(&v).unwrap();
        assert_eq!(p.repo, "o/r");
        assert_eq!(p.number, 42);
        assert_eq!(p.node_id, "PR_node");
        assert_eq!(p.head_sha, "abc");
        assert_eq!(p.base_ref, "main");
        assert!(p.draft);
    }

    #[test]
    fn parse_status_maps_to_check() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{
                "repository": { "full_name": "o/r" },
                "sha": "deadbeef", "context": "ci/test", "state": "success",
                "target_url": "https://example.test/run"
            }"#,
        )
        .unwrap();
        let c = parse_status(&v).unwrap();
        assert_eq!(c.head_sha, "deadbeef");
        assert_eq!(c.external_id, "status:ci/test");
        assert_eq!(c.conclusion.as_deref(), Some("success"));
    }

    #[test]
    fn parse_check_run_extracts_sha() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{
                "repository": { "full_name": "o/r" },
                "check_run": {
                    "id": 99, "head_sha": "cafe", "name": "build",
                    "status": "completed", "conclusion": "failure"
                }
            }"#,
        )
        .unwrap();
        let c = parse_check("check_run", &v).unwrap();
        assert_eq!(c.head_sha, "cafe");
        assert_eq!(c.external_id, "99");
        assert_eq!(c.conclusion.as_deref(), Some("failure"));
    }

    /// DB-gated end-to-end mapping: a `pull_request` event routed through
    /// [`dispatch`] lands a row in `github.pulls`. Mirrors the `upsert_*`
    /// integration tests — point `TEST_DATABASE_URL` at a throwaway Postgres
    /// and run with `--ignored`.
    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL"]
    async fn pull_request_event_upserts_a_pull() {
        use sqlx::postgres::PgPoolOptions;
        use sqlx::{Executor, Row};

        let Ok(url) = std::env::var("TEST_DATABASE_URL") else { return };
        let pool = PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
        pool.execute("DROP SCHEMA IF EXISTS github CASCADE").await.unwrap();
        pool.execute(
            "CREATE TABLE IF NOT EXISTS public.users \
             (id UUID PRIMARY KEY DEFAULT gen_random_uuid())",
        )
        .await
        .unwrap();
        pool.execute("TRUNCATE public.users CASCADE").await.unwrap();
        crate::migrate(&pool).await.unwrap();

        let user_id: Uuid = pool
            .fetch_one("INSERT INTO public.users DEFAULT VALUES RETURNING id")
            .await
            .unwrap()
            .get(0);
        let connector_id: Uuid = pool
            .fetch_one(
                sqlx::query(
                    "INSERT INTO github.connectors (user_id, name, credential_kind, encrypted_credential) \
                     VALUES ($1, 'test', 'pat', 'x') RETURNING id",
                )
                .bind(user_id),
            )
            .await
            .unwrap()
            .get(0);

        let (tx, _rx) = tokio::sync::broadcast::channel(8);
        let state = GithubState {
            pool: pool.clone(),
            events: tx,
            pr_cache: cctui_proto::classifier::PrStatusCache::new(),
        };
        let body = br#"{
            "repository": { "full_name": "o/r" },
            "pull_request": {
                "node_id": "PR_node", "number": 42, "title": "t",
                "state": "open", "merged": false, "draft": false,
                "user": { "login": "me" },
                "head": { "sha": "abc", "ref": "feat" },
                "base": { "ref": "main" },
                "created_at": "2026-06-17T00:00:00Z",
                "updated_at": "2026-06-17T01:00:00Z"
            }
        }"#;

        let res = dispatch(&state, connector_id, "pull_request", body).await;
        assert!(matches!(res, Ok(Handled::Accepted)));
        let n: i64 = pool
            .fetch_one("SELECT count(*) FROM github.pulls WHERE number = 42")
            .await
            .unwrap()
            .get(0);
        assert_eq!(n, 1);
    }

    #[test]
    fn missing_pull_request_is_bad_payload() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{ "repository": { "full_name": "o/r" } }"#).unwrap();
        assert!(parse_pull(&v).is_none());
    }
}
