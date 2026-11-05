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
             OR EXISTS (SELECT 1 FROM session_tokens WHERE encrypted_token IS NOT NULL) \
             OR EXISTS (SELECT 1 FROM cctuiverse_links WHERE encrypted_private_key IS NOT NULL) \
             OR EXISTS (SELECT 1 FROM instance_settings \
                        WHERE key = 'speech' AND value ? 'encrypted_key') \
             OR EXISTS (SELECT 1 FROM session_message_queue WHERE sealed_body IS NOT NULL) \
             OR EXISTS (SELECT 1 FROM sessions \
                        WHERE status = 'draft' AND metadata ? 'draft_sealed_prompt')",
    )
    .fetch_one(db)
    .await
}

#[cfg(test)]
mod tests {
    use super::has_vault_data;

    async fn clear_vault_rows(tx: &mut sqlx::PgConnection) {
        for sql in [
            "LOCK TABLE session_message_queue IN SHARE MODE",
            "DELETE FROM api_keys",
            "DELETE FROM session_tokens",
            "DELETE FROM cctuiverse_links",
            "DELETE FROM account_providers",
            "DELETE FROM accounts",
            "DELETE FROM instance_settings WHERE key = 'speech'",
            "DELETE FROM session_message_queue WHERE sealed_body IS NOT NULL",
            "UPDATE sessions SET metadata = metadata - 'draft_sealed_prompt' \
             WHERE metadata ? 'draft_sealed_prompt'",
        ] {
            sqlx::query(sql).execute(&mut *tx).await.unwrap();
        }
    }

    #[tokio::test]
    async fn has_vault_data_sees_a_sealed_queued_message_alone() {
        let name = "has_vault_data_sees_a_sealed_queued_message_alone";
        let Some(url) = crate::routes::gateway::test_db_url(name) else {
            return;
        };
        let pool = crate::db::connect(&url).await.expect("connect test db");
        let mut tx = pool.begin().await.unwrap();
        clear_vault_rows(&mut tx).await;
        let sid = format!("{name}-{}", uuid::Uuid::new_v4());
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, working_dir, status) \
             VALUES ($1, 'm', '/w', 'idle')",
        )
        .bind(&sid)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO session_message_queue (session_id, body, deliver_at, next_attempt_at) \
             VALUES ($1, 'pw [REDACTED:user]', now(), now())",
        )
        .bind(&sid)
        .execute(&mut *tx)
        .await
        .unwrap();
        assert!(!has_vault_data(&mut *tx).await.unwrap(), "an unsealed body is not vault data");
        sqlx::query("UPDATE session_message_queue SET sealed_body = 'v1:00' WHERE session_id = $1")
            .bind(&sid)
            .execute(&mut *tx)
            .await
            .unwrap();
        assert!(has_vault_data(&mut *tx).await.unwrap());
        tx.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn has_vault_data_sees_a_sealed_draft_prompt_alone() {
        let name = "has_vault_data_sees_a_sealed_draft_prompt_alone";
        let Some(url) = crate::routes::gateway::test_db_url(name) else {
            return;
        };
        let pool = crate::db::connect(&url).await.expect("connect test db");
        let mut tx = pool.begin().await.unwrap();
        clear_vault_rows(&mut tx).await;
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, working_dir, status, metadata) \
             VALUES ($1, 'm', '/w', 'draft', '{\"draft_sealed_prompt\": \"v1:00\"}')",
        )
        .bind(format!("{name}-{}", uuid::Uuid::new_v4()))
        .execute(&mut *tx)
        .await
        .unwrap();
        assert!(has_vault_data(&mut *tx).await.unwrap());
        tx.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn has_vault_data_sees_encrypted_rows() {
        let Some(url) = crate::routes::gateway::test_db_url("has_vault_data_sees_encrypted_rows")
        else {
            return;
        };
        let pool = crate::db::connect(&url).await.expect("connect test db");
        let mut tx = pool.begin().await.unwrap();
        clear_vault_rows(&mut tx).await;
        assert!(!has_vault_data(&mut *tx).await.unwrap());
        sqlx::query(
            "INSERT INTO instance_settings (key, value) VALUES ('speech', '{\"enabled\": false}')",
        )
        .execute(&mut *tx)
        .await
        .unwrap();
        assert!(!has_vault_data(&mut *tx).await.unwrap());
        sqlx::query(
            "UPDATE instance_settings SET value = '{\"encrypted_key\": \"v1:00\"}' WHERE key = 'speech'",
        )
        .execute(&mut *tx)
        .await
        .unwrap();
        assert!(has_vault_data(&mut *tx).await.unwrap());
        sqlx::query("DELETE FROM instance_settings WHERE key = 'speech'")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO api_keys (name, encrypted_key) VALUES ('k', 'v1:00')")
            .execute(&mut *tx)
            .await
            .unwrap();
        assert!(has_vault_data(&mut *tx).await.unwrap());
        tx.rollback().await.unwrap();
    }
}
