use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use kagi::{
    Ceremony, CeremonyStore, CredentialStore, Error, Kagi, Limits, Passkey, Removal, ResidentKey,
    Settings, StoreError, StoredPasskey, Url, Uuid,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

pub const SESSION_KIND: &str = "passkey";

pub type Passkeys = Kagi<Uuid, PgCeremonies>;

/// Longer than the authenticator timeout the options ask for, so the person
/// meets the authenticator's own deadline first.
const CEREMONY_TTL: Duration = Duration::from_mins(10);

#[must_use]
pub fn build(
    external_url: &str,
    rp_id: Option<&str>,
    pool: PgPool,
    vault_key: &[u8],
) -> Option<Passkeys> {
    let url = Url::parse(external_url.trim_end_matches('/'))
        .map_err(|e| tracing::warn!("passkeys off: CCTUI_EXTERNAL_URL is not a URL: {e}"))
        .ok()?;
    let decoy_key = decoy_key(vault_key);
    let settings = Settings {
        rp_id,
        resident_key: ResidentKey::Required,
        ..Settings::new(&url, "cctui", &decoy_key)
    };
    let limits = Limits { ttl: CEREMONY_TTL, ..Limits::default() };
    Kagi::new(settings, PgCeremonies::new(pool, limits))
        .map_err(|e| tracing::info!(%url, "passkeys off: {e}"))
        .ok()
}

fn decoy_key(vault_key: &[u8]) -> [u8; 32] {
    Sha256::new().chain_update(b"cctui passkey decoys\0").chain_update(vault_key).finalize().into()
}

/// Times come from the database clock so every replica agrees on expiry.
pub struct PgCeremonies {
    pool: PgPool,
    limits: Limits,
}

impl PgCeremonies {
    #[must_use]
    pub const fn new(pool: PgPool, limits: Limits) -> Self {
        Self { pool, limits }
    }

    async fn insert(
        &self,
        address: Option<IpAddr>,
        ceremony: &Ceremony<Uuid>,
    ) -> Result<Result<String, Error>, sqlx::Error> {
        let address = address.map(|ip| kagi::network(ip).to_string());
        let ttl = self.limits.ttl.as_secs_f64();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtext('webauthn_ceremonies'), \
                                          hashtext(coalesce($1, '')))",
        )
        .bind(&address)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM webauthn_ceremonies WHERE expires_at <= now()")
            .execute(&mut *tx)
            .await?;
        let (held, retry_after): (i64, Option<f64>) = sqlx::query_as(
            "SELECT count(*), extract(epoch FROM min(expires_at) - now())::float8 \
             FROM webauthn_ceremonies WHERE address IS NOT DISTINCT FROM $1",
        )
        .bind(&address)
        .fetch_one(&mut *tx)
        .await?;
        if usize::try_from(held).unwrap_or(usize::MAX) >= self.limits.per_address {
            let retry_after = Duration::from_secs_f64(retry_after.unwrap_or(ttl).clamp(1.0, ttl));
            return Ok(Err(Error::TooMany { retry_after }));
        }
        let total: i64 = sqlx::query_scalar("SELECT count(*) FROM webauthn_ceremonies")
            .fetch_one(&mut *tx)
            .await?;
        if usize::try_from(total).unwrap_or(usize::MAX) >= self.limits.global {
            return Ok(Err(Error::Busy));
        }
        let id = match kagi::ceremony_id() {
            Ok(id) => id,
            Err(e) => return Ok(Err(e)),
        };
        let state = match serde_json::to_value(ceremony) {
            Ok(state) => state,
            Err(e) => return Ok(Err(Error::Internal(e.to_string()))),
        };
        sqlx::query(
            "INSERT INTO webauthn_ceremonies (key, purpose, address, ceremony, expires_at) \
             VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5))",
        )
        .bind(kagi::ceremony_key(&id).as_slice())
        .bind(ceremony.purpose().as_str())
        .bind(&address)
        .bind(state)
        .bind(ttl)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Ok(id))
    }
}

impl CeremonyStore<Uuid> for PgCeremonies {
    async fn begin(
        &self,
        address: Option<IpAddr>,
        ceremony: Ceremony<Uuid>,
    ) -> Result<String, Error> {
        self.insert(address, &ceremony).await.map_err(|e| Error::Store(e.into()))?
    }

    async fn take(&self, id: &str) -> Result<Option<Ceremony<Uuid>>, Error> {
        let row: Option<(Value, bool)> = sqlx::query_as(
            "DELETE FROM webauthn_ceremonies WHERE key = $1 \
             RETURNING ceremony, expires_at > now()",
        )
        .bind(kagi::ceremony_key(id).as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Store(e.into()))?;
        match row {
            Some((state, true)) => serde_json::from_value(state)
                .map(Some)
                .map_err(|e| Error::Internal(format!("stored passkey ceremony: {e}"))),
            _ => Ok(None),
        }
    }
}

/// The user handle is `users.id` itself: every stored passkey was registered with it.
pub struct PgCredentials<'a> {
    pool: &'a PgPool,
    revoked: Mutex<Vec<String>>,
}

impl<'a> PgCredentials<'a> {
    #[must_use]
    pub const fn new(pool: &'a PgPool) -> Self {
        Self { pool, revoked: Mutex::new(Vec::new()) }
    }

    pub fn revoked(&self) -> Vec<String> {
        std::mem::take(&mut *self.revoked.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

type Row = StoredPasskey<Uuid, Uuid>;

fn row((id, user, passkey): (Uuid, Uuid, Value)) -> Result<Row, StoreError> {
    Ok(StoredPasskey { id, user, passkey: serde_json::from_value(passkey)? })
}

impl CredentialStore for PgCredentials<'_> {
    type User = Uuid;
    type PasskeyId = Uuid;
    type Error = StoreError;

    fn user_handle(&self, user: &Uuid) -> Uuid {
        *user
    }

    async fn passkeys(&self, user: &Uuid) -> Result<Vec<Row>, StoreError> {
        let rows: Vec<(Uuid, Uuid, Value)> = sqlx::query_as(
            "SELECT id, user_id, passkey FROM webauthn_credentials \
             WHERE user_id = $1 ORDER BY created_at",
        )
        .bind(user)
        .fetch_all(self.pool)
        .await?;
        rows.into_iter().map(row).collect()
    }

    async fn find(&self, credential_id: &[u8]) -> Result<Option<Row>, StoreError> {
        let found: Option<(Uuid, Uuid, Value)> = sqlx::query_as(
            "SELECT id, user_id, passkey FROM webauthn_credentials WHERE credential_id = $1",
        )
        .bind(credential_id)
        .fetch_optional(self.pool)
        .await?;
        found.map(row).transpose()
    }

    async fn add(&self, user: &Uuid, passkey: &Passkey, label: &str) -> Result<Uuid, StoreError> {
        Ok(sqlx::query_scalar(
            "INSERT INTO webauthn_credentials (user_id, credential_id, passkey, label) \
             VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(user)
        .bind(passkey.cred_id().as_slice())
        .bind(serde_json::to_value(passkey)?)
        .bind(label)
        .fetch_one(self.pool)
        .await?)
    }

    async fn update(&self, id: &Uuid, passkey: &Passkey) -> Result<(), StoreError> {
        sqlx::query("UPDATE webauthn_credentials SET passkey = $1 WHERE id = $2")
            .bind(serde_json::to_value(passkey)?)
            .bind(id)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    /// Revokes the sign-in keys the passkey minted in the same transaction,
    /// including those minted before keys recorded their passkey.
    async fn remove(&self, user: &Uuid, id: &Uuid, keep_last: bool) -> Result<Removal, StoreError> {
        let mut tx = self.pool.begin().await?;
        let held: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM webauthn_credentials WHERE user_id = $1 FOR UPDATE")
                .bind(user)
                .fetch_all(&mut *tx)
                .await?;
        if !held.contains(id) {
            return Ok(Removal::NotFound);
        }
        if keep_last && held.len() == 1 {
            return Ok(Removal::Last);
        }
        sqlx::query("DELETE FROM webauthn_credentials WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let hashes: Vec<String> = sqlx::query_scalar(
            "UPDATE auth_keys SET revoked_at = now() \
             WHERE user_id = $1 AND kind = $2 AND revoked_at IS NULL \
               AND (passkey_id = $3 OR passkey_id IS NULL) \
             RETURNING key_hash",
        )
        .bind(user)
        .bind(SESSION_KIND)
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        self.revoked.lock().unwrap_or_else(PoisonError::into_inner).extend(hashes);
        Ok(Removal::Removed { remaining: held.len() - 1 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool() -> PgPool {
        PgPool::connect_lazy("postgres://localhost/unused").unwrap()
    }

    #[tokio::test]
    async fn https_and_localhost_yield_a_relying_party() {
        for url in
            ["https://cctui.example.com", "https://cctui.example.com/", "http://localhost:8700"]
        {
            assert!(build(url, None, pool(), b"k").is_some(), "{url}");
        }
    }

    #[tokio::test]
    async fn an_insecure_or_ip_url_disables_passkeys() {
        for url in ["http://cctui.example.com", "http://127.0.0.1:8700", "https://10.0.0.1", "nope"]
        {
            assert!(build(url, None, pool(), b"k").is_none(), "{url}");
        }
    }

    #[tokio::test]
    async fn rp_id_override_must_still_cover_the_origin() {
        assert!(build("https://cctui.example.com", Some("example.com"), pool(), b"k").is_some());
        assert!(build("https://cctui.example.com", Some("elsewhere.test"), pool(), b"k").is_none());
    }

    #[test]
    fn the_decoy_key_is_stable_and_follows_the_vault_key() {
        assert_eq!(decoy_key(b"vault"), decoy_key(b"vault"));
        assert_ne!(decoy_key(b"vault"), decoy_key(b"other"));
        assert_ne!(decoy_key(b""), [0; 32]);
    }

    fn kagi_with(pool: &PgPool, limits: Limits) -> Passkeys {
        let url = Url::parse("https://cctui.example.com").unwrap();
        let key = decoy_key(b"test");
        Kagi::new(Settings::new(&url, "cctui", &key), PgCeremonies::new(pool.clone(), limits))
            .unwrap()
    }

    fn address() -> IpAddr {
        let bytes = Uuid::new_v4().into_bytes();
        IpAddr::from([10, bytes[0], bytes[1], bytes[2]])
    }

    fn attestation() -> kagi::RegisterPublicKeyCredential {
        serde_json::from_value(serde_json::json!({
            "id": "AQID", "rawId": "AQID", "type": "public-key",
            "response": { "attestationObject": "oA", "clientDataJSON": "e30" },
            "clientExtensionResults": {},
        }))
        .unwrap()
    }

    async fn stored(pool: &PgPool, id: &str) -> Option<(String, Option<String>, Value)> {
        sqlx::query_as("SELECT purpose, address, ceremony FROM webauthn_ceremonies WHERE key = $1")
            .bind(kagi::ceremony_key(id).as_slice())
            .fetch_optional(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_ceremony_is_stored_by_hash_and_taken_once_on_any_replica() {
        let Some(url) = cctui_server_kernel::test_support::db_url(
            "a_ceremony_is_stored_by_hash_and_taken_once_on_any_replica",
        ) else {
            return;
        };
        let (one, two) =
            (PgPool::connect(&url).await.unwrap(), PgPool::connect(&url).await.unwrap());
        let (first, second) =
            (kagi_with(&one, Limits::default()), kagi_with(&two, Limits::default()));
        let ip = address();
        let id = first.start_discoverable(Some(ip), false).await.unwrap().ceremony;
        let (purpose, held_by, state) = stored(&one, &id).await.expect("stored");
        assert_eq!(purpose, "sign-in");
        assert_eq!(held_by, Some(ip.to_string()));
        assert!(!state.to_string().contains(&id));
        let none: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM webauthn_ceremonies WHERE key = convert_to($1, 'UTF8')",
        )
        .bind(&id)
        .fetch_one(&one)
        .await
        .unwrap();
        assert_eq!(none, 0);

        assert!(second.ceremonies().take(&id).await.unwrap().is_some(), "the other replica");
        assert!(first.ceremonies().take(&id).await.unwrap().is_none(), "single use");
        assert!(second.ceremonies().take(&id).await.unwrap().is_none());
        assert!(first.ceremonies().take("not an id").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_ceremony_finishes_only_its_purpose_and_never_after_it_expires() {
        let Some(url) = cctui_server_kernel::test_support::db_url(
            "a_ceremony_finishes_only_its_purpose_and_never_after_it_expires",
        ) else {
            return;
        };
        let pool = PgPool::connect(&url).await.unwrap();
        let passkeys = kagi_with(&pool, Limits::default());
        let credentials = PgCredentials::new(&pool);

        let id = passkeys.start_discoverable(Some(address()), false).await.unwrap().ceremony;
        let wrong = passkeys.finish_registration(&credentials, &id, "s", &attestation(), "x").await;
        assert!(matches!(wrong, Err(Error::Purpose)), "{wrong:?}");
        assert!(stored(&pool, &id).await.is_none(), "taken by the wrong finish");

        let id = passkeys.start_discoverable(Some(address()), false).await.unwrap().ceremony;
        sqlx::query(
            "UPDATE webauthn_ceremonies SET expires_at = now() - interval '1 second' WHERE key = $1",
        )
        .bind(kagi::ceremony_key(&id).as_slice())
        .execute(&pool)
        .await
        .unwrap();
        assert!(passkeys.ceremonies().take(&id).await.unwrap().is_none(), "expired");
        assert!(stored(&pool, &id).await.is_none(), "an expired take still removes it");
    }

    #[tokio::test]
    async fn a_full_address_is_refused_without_evicting_anyone() {
        let Some(url) = cctui_server_kernel::test_support::db_url(
            "a_full_address_is_refused_without_evicting_anyone",
        ) else {
            return;
        };
        let pool = PgPool::connect(&url).await.unwrap();
        let passkeys = kagi_with(&pool, Limits { per_address: 2, ..Limits::default() });
        let (mine, theirs) = (address(), address());
        let held = [
            passkeys.start_discoverable(Some(mine), false).await.unwrap().ceremony,
            passkeys.start_discoverable(Some(mine), false).await.unwrap().ceremony,
        ];
        match passkeys.start_discoverable(Some(mine), false).await {
            Err(Error::TooMany { retry_after }) => {
                assert!(retry_after >= Duration::from_secs(1) && retry_after <= CEREMONY_TTL);
            }
            other => panic!("{other:?}"),
        }
        assert!(passkeys.start_discoverable(Some(theirs), false).await.is_ok());
        for id in &held {
            assert!(passkeys.ceremonies().take(id).await.unwrap().is_some());
        }
        assert!(passkeys.start_discoverable(Some(mine), false).await.is_ok());

        let full = kagi_with(&pool, Limits { global: 1, ..Limits::default() });
        assert!(matches!(full.start_discoverable(Some(address()), false).await, Err(Error::Busy)));
    }
}
