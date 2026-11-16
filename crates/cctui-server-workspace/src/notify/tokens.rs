use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::config::Config;

pub const MAX_LIFETIME_HOURS: i32 = 24;

const TOKEN_BYTES: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct TokenRow {
    pub notification_id: Uuid,
    pub action: String,
    pub channel: String,
    pub usable: bool,
}

fn random_bytes() -> [u8; TOKEN_BYTES] {
    let mut seed = Sha256::new();
    for _ in 0..3 {
        seed.update(Uuid::new_v4().as_bytes());
    }
    seed.finalize().into()
}

fn digest(raw: &[u8]) -> Vec<u8> {
    Sha256::digest(raw).to_vec()
}

#[must_use]
pub fn hash(token: &str) -> Option<Vec<u8>> {
    let raw = URL_SAFE_NO_PAD.decode(token.trim()).ok()?;
    (raw.len() == TOKEN_BYTES).then(|| digest(&raw))
}

const MINT_SQL: &str = "\
    INSERT INTO notification_action_tokens \
        (token_hash, notification_id, delivery_id, action, channel, expires_at) \
    SELECT $1, n.id, $3, $4, $5, \
        LEAST($6, COALESCE(n.expires_at, $6), now() + make_interval(hours => $7)) \
    FROM notifications n \
    WHERE n.id = $2 AND n.state IN ('pending', 'delivered', 'read') \
      AND jsonb_typeof(n.actions) = 'array' \
      AND EXISTS ( \
          SELECT 1 FROM jsonb_array_elements(n.actions) a \
          WHERE a ->> 'id' = $4 AND a -> 'requires_login' IS DISTINCT FROM 'true'::jsonb)";

/// The URL-safe token for one action on one notice; only its SHA-256 is stored.
///
/// The token expires at the earliest of `expires_at`, the notice's own expiry and
/// [`MAX_LIFETIME_HOURS`]. `None` when the notice is closed or unknown, does not offer
/// `action`, or the action `requires_login`.
pub async fn mint<'e>(
    conn: impl sqlx::PgExecutor<'e>,
    notification_id: Uuid,
    delivery_id: Option<Uuid>,
    action: &str,
    channel: &str,
    expires_at: DateTime<Utc>,
) -> sqlx::Result<Option<String>> {
    let raw = random_bytes();
    let minted = sqlx::query(MINT_SQL)
        .bind(digest(&raw))
        .bind(notification_id)
        .bind(delivery_id)
        .bind(action)
        .bind(channel)
        .bind(expires_at)
        .bind(MAX_LIFETIME_HOURS)
        .execute(conn)
        .await?;
    Ok((minted.rows_affected() == 1).then(|| URL_SAFE_NO_PAD.encode(raw)))
}

fn base(config: &Config) -> &str {
    config.external_url.trim_end_matches('/')
}

/// Where a POST runs the action: ntfy `http` buttons.
#[must_use]
pub fn action_url(config: &Config, token: &str) -> String {
    format!("{}/api/v1/notifications/actions/{token}", base(config))
}

/// The confirm page: for channels whose links get prefetched (messengers, mail).
#[cfg_attr(not(test), allow(dead_code))]
#[must_use]
pub fn confirm_url(config: &Config, token: &str) -> String {
    format!("{}/n/{token}", base(config))
}

pub async fn peek(pool: &PgPool, hash: &[u8]) -> sqlx::Result<Option<TokenRow>> {
    sqlx::query_as(
        "SELECT notification_id, action, channel, \
           (used_at IS NULL AND expires_at > now()) AS usable \
         FROM notification_action_tokens WHERE token_hash = $1",
    )
    .bind(hash)
    .fetch_optional(pool)
    .await
}

/// Single use: only the first caller gets the row back.
pub async fn burn(pool: &PgPool, hash: &[u8]) -> sqlx::Result<Option<TokenRow>> {
    sqlx::query_as(
        "UPDATE notification_action_tokens SET used_at = now() \
         WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now() \
         RETURNING notification_id, action, channel, TRUE AS usable",
    )
    .bind(hash)
    .fetch_optional(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minted_token_round_trips_to_its_stored_hash() {
        let raw = random_bytes();
        let token = URL_SAFE_NO_PAD.encode(raw);
        assert_eq!(token.len(), 43);
        assert_eq!(hash(&token), Some(digest(&raw)));
        assert_ne!(random_bytes(), raw);
    }

    #[test]
    fn junk_is_not_a_token() {
        assert_eq!(hash(""), None);
        assert_eq!(hash("not base64!"), None);
        assert_eq!(hash(&URL_SAFE_NO_PAD.encode([0u8; 16])), None);
    }

    #[test]
    fn urls_hang_under_the_external_url() {
        let mut config = Config::for_test(vec![]);
        config.external_url = "https://cctui.example/".into();
        assert_eq!(
            action_url(&config, "tok"),
            "https://cctui.example/api/v1/notifications/actions/tok"
        );
        assert_eq!(confirm_url(&config, "tok"), "https://cctui.example/n/tok");
    }
}
