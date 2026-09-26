use std::sync::OnceLock;

pub use cctui_crypto::{decrypt, encrypt};

static KEY: OnceLock<Vec<u8>> = OnceLock::new();

/// Pin the key resolved at startup (possibly a tolerated legacy key).
pub fn install_vault_key(key: Vec<u8>) {
    let _ = KEY.set(key);
}

/// The startup-resolved key; before [`install_vault_key`] an unset or
/// invalid `CCTUI_VAULT_KEY` panics rather than degrading to pass-through.
#[must_use]
pub fn vault_key() -> Vec<u8> {
    KEY.get().cloned().unwrap_or_else(cctui_crypto::vault_key_required)
}

/// Whether any column the vault writes already holds a value.
pub async fn has_vault_data<'e, E: sqlx::PgExecutor<'e>>(db: E) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM api_keys) \
             OR EXISTS (SELECT 1 FROM account_providers \
                        WHERE encrypted_refresh_token IS NOT NULL \
                           OR encrypted_access_token IS NOT NULL) \
             OR EXISTS (SELECT 1 FROM accounts WHERE env_json IS NOT NULL) \
             OR EXISTS (SELECT 1 FROM session_tokens WHERE encrypted_token IS NOT NULL)",
    )
    .fetch_one(db)
    .await
}

#[cfg(test)]
mod tests {
    use super::has_vault_data;

    #[tokio::test]
    async fn has_vault_data_sees_encrypted_rows() {
        let Some(url) = crate::routes::gateway::test_db_url("has_vault_data_sees_encrypted_rows")
        else {
            return;
        };
        let pool = crate::db::connect(&url).await.expect("connect test db");
        let mut tx = pool.begin().await.unwrap();
        for sql in [
            "DELETE FROM api_keys",
            "DELETE FROM session_tokens",
            "DELETE FROM account_providers",
            "DELETE FROM accounts",
        ] {
            sqlx::query(sql).execute(&mut *tx).await.unwrap();
        }
        assert!(!has_vault_data(&mut *tx).await.unwrap());
        sqlx::query("INSERT INTO api_keys (name, encrypted_key) VALUES ('k', 'v1:00')")
            .execute(&mut *tx)
            .await
            .unwrap();
        assert!(has_vault_data(&mut *tx).await.unwrap());
        tx.rollback().await.unwrap();
    }
}
