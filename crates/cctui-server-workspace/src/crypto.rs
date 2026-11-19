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
             OR EXISTS (SELECT 1 FROM inbox_events WHERE sealed_body IS NOT NULL) \
             OR EXISTS (SELECT 1 FROM instance_settings \
                        WHERE key = 'web_push_vapid' AND value ? 'encrypted_key') \
             OR EXISTS (SELECT 1 FROM notification_endpoints WHERE config ? 'token_sealed') \
             OR EXISTS (SELECT 1 FROM sessions \
                        WHERE status = 'draft' AND metadata ? 'draft_sealed_prompt') \
             OR EXISTS (SELECT 1 FROM sessions WHERE metadata ? 'draft_sealed_env') \
             OR EXISTS (SELECT 1 FROM presets WHERE sealed_env IS NOT NULL)",
    )
    .fetch_one(db)
    .await
}

#[cfg(test)]
mod tests {
    use super::has_vault_data;

    async fn clear_vault_rows(tx: &mut sqlx::PgConnection) {
        for sql in [
            "SELECT pg_advisory_xact_lock(hashtext('crypto::tests::clear_vault_rows'))",
            "LOCK TABLE inbox_events IN SHARE MODE",
            "DELETE FROM api_keys",
            "DELETE FROM session_tokens",
            "DELETE FROM cctuiverse_links",
            "DELETE FROM account_providers",
            "DELETE FROM accounts",
            "DELETE FROM instance_settings WHERE key IN ('speech', 'web_push_vapid')",
            "DELETE FROM notification_endpoints WHERE config ? 'token_sealed'",
            "DELETE FROM inbox_events WHERE sealed_body IS NOT NULL",
            "UPDATE sessions SET metadata = metadata - 'draft_sealed_prompt' \
             WHERE metadata ? 'draft_sealed_prompt'",
            "UPDATE sessions SET metadata = metadata - 'draft_sealed_env' \
             WHERE metadata ? 'draft_sealed_env'",
            "UPDATE presets SET sealed_env = NULL WHERE sealed_env IS NOT NULL",
        ] {
            sqlx::query(sql).execute(&mut *tx).await.unwrap();
        }
    }

    #[tokio::test]
    async fn has_vault_data_sees_a_sealed_inbox_event_alone() {
        let name = "has_vault_data_sees_a_sealed_inbox_event_alone";
        let Some(url) = crate::routes::gateway::test_db_url(name) else {
            return;
        };
        let pool = crate::db::connect(&url).await.expect("connect test db");
        let mut tx = pool.begin().await.unwrap();
        clear_vault_rows(&mut tx).await;
        let uid = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("{name}-{uid}"))
            .bind(format!("h-{uid}"))
            .execute(&mut *tx)
            .await
            .unwrap();
        let lane: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO inbox_lanes (user_id, key) VALUES ($1, $2) RETURNING id",
        )
        .bind(uid)
        .bind(format!("session:{name}-{uid}"))
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO inbox_events (lane_id, user_id, source, body) \
             VALUES ($1, $2, 'human', 'pw [REDACTED:user]')",
        )
        .bind(lane)
        .bind(uid)
        .execute(&mut *tx)
        .await
        .unwrap();
        assert!(!has_vault_data(&mut *tx).await.unwrap(), "an unsealed body is not vault data");
        sqlx::query("UPDATE inbox_events SET sealed_body = 'v1:00' WHERE lane_id = $1")
            .bind(lane)
            .execute(&mut *tx)
            .await
            .unwrap();
        assert!(has_vault_data(&mut *tx).await.unwrap());
        tx.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn has_vault_data_sees_a_sealed_vapid_key_alone() {
        let name = "has_vault_data_sees_a_sealed_vapid_key_alone";
        let Some(url) = crate::routes::gateway::test_db_url(name) else {
            return;
        };
        let pool = crate::db::connect(&url).await.expect("connect test db");
        let mut tx = pool.begin().await.unwrap();
        clear_vault_rows(&mut tx).await;
        sqlx::query("INSERT INTO instance_settings (key, value) VALUES ('web_push_vapid', '{}')")
            .execute(&mut *tx)
            .await
            .unwrap();
        assert!(!has_vault_data(&mut *tx).await.unwrap(), "an empty setting is not vault data");
        sqlx::query(
            "UPDATE instance_settings SET value = '{\"encrypted_key\": \"v1:00\"}' \
             WHERE key = 'web_push_vapid'",
        )
        .execute(&mut *tx)
        .await
        .unwrap();
        assert!(has_vault_data(&mut *tx).await.unwrap());
        tx.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn has_vault_data_sees_a_sealed_ntfy_token_alone() {
        let name = "has_vault_data_sees_a_sealed_ntfy_token_alone";
        let Some(url) = crate::routes::gateway::test_db_url(name) else {
            return;
        };
        let pool = crate::db::connect(&url).await.expect("connect test db");
        let mut tx = pool.begin().await.unwrap();
        clear_vault_rows(&mut tx).await;
        let uid = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("{name}-{uid}"))
            .bind(format!("h-{uid}"))
            .execute(&mut *tx)
            .await
            .unwrap();
        let endpoint: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO notification_endpoints (user_id, channel, config) \
             VALUES ($1, 'ntfy', '{\"url\": \"https://ntfy.example\", \"topic\": \"t\"}') \
             RETURNING id",
        )
        .bind(uid)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert!(!has_vault_data(&mut *tx).await.unwrap(), "an unsealed token is not vault data");
        sqlx::query(
            "UPDATE notification_endpoints \
             SET config = config || '{\"token_sealed\": \"v1:00\"}' WHERE id = $1",
        )
        .bind(endpoint)
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
    async fn has_vault_data_sees_a_sealed_draft_env_alone() {
        let name = "has_vault_data_sees_a_sealed_draft_env_alone";
        let Some(url) = crate::routes::gateway::test_db_url(name) else {
            return;
        };
        let pool = crate::db::connect(&url).await.expect("connect test db");
        let mut tx = pool.begin().await.unwrap();
        clear_vault_rows(&mut tx).await;
        assert!(!has_vault_data(&mut *tx).await.unwrap());
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, working_dir, status, metadata) \
             VALUES ($1, 'm', '/w', 'queued', '{\"draft_sealed_env\": \"v1:00\"}')",
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
