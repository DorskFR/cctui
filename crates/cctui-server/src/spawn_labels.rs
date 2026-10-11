//! Labels chosen at spawn time, and the rule every attach path shares.
//!
//! A spawn that carries `label_ids` leaves an intent row keyed by its spawn key
//! (the pre-minted session id for claude-code, the command id for adapters
//! that mint their own id), the same key `auto_archive` uses. When the daemon
//! registers the session, the intent is claimed and the labels are attached.
//! This is what lets a draft launch keep its labels: the launch returns before
//! the worker registers, so nothing on the client side knows the session id.
//!
//! Labels are per-user vocabulary, and a label on a session always lives in
//! the session owner's vocabulary. Which labels a spawn may carry is the
//! caller's visibility (its own, or any for an admin); each one is then mapped
//! onto the owner's label of the same name, created when missing, once at
//! spawn time ([`for_spawn`]) and again at attach time, so an intent recorded
//! under older rules still lands. An admin spawning on
//! a user's machine with a label from the admin picker therefore lands that
//! user's label, instead of nothing.
//!
//! A label deleted between the spawn and the registration is skipped, never
//! an error: labels are best-effort and must not fail the spawn.

use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

/// Unclaimed intents older than this are dropped: the spawn never registered.
const INTENT_TTL_SECS: i64 = 6 * 3600;

/// The label ids of a spawn request that parse as uuids, deduplicated in their
/// original order. A malformed id is dropped rather than refused, like a
/// deleted label.
pub fn parse_label_ids(raw: &[String]) -> Vec<Uuid> {
    let mut out: Vec<Uuid> = Vec::new();
    for id in raw.iter().filter_map(|s| Uuid::parse_str(s.trim()).ok()) {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// The labels of `ids` the caller may put on a session: its own labels, or any
/// label for an admin (`visible_to = None`, the god-view). Unknown ids drop.
async fn visible_label_ids(
    conn: &mut PgConnection,
    ids: &[Uuid],
    visible_to: Option<Uuid>,
) -> Result<Vec<Uuid>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let found: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM labels WHERE id = ANY($1) AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(ids)
    .bind(visible_to)
    .fetch_all(&mut *conn)
    .await?;
    Ok(ids.iter().copied().filter(|id| found.contains(id)).collect())
}

/// The user a session's labels belong to: the session's owner, else its
/// machine's. `None` when the row is gone or unowned.
pub async fn session_owner(
    conn: &mut PgConnection,
    session_id: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    let owner: Option<Option<Uuid>> = sqlx::query_scalar(
        "SELECT COALESCE(s.user_id, m.user_id) FROM sessions s \
           LEFT JOIN machines m ON m.id = s.machine_uuid WHERE s.id = $1",
    )
    .bind(session_id)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(owner.flatten())
}

/// Map label ids onto `owner`'s vocabulary, in order and deduplicated: an
/// owner's label is kept, another user's label is swapped for the owner's
/// label of the same (case-insensitive) name, created with the source color
/// when the owner has none. Unknown ids drop.
pub async fn owner_label_ids(
    conn: &mut PgConnection,
    owner: Uuid,
    ids: &[Uuid],
) -> Result<Vec<Uuid>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<(Uuid, String, String, Uuid)> =
        sqlx::query_as("SELECT id, name, color, user_id FROM labels WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(&mut *conn)
            .await?;
    let mut out: Vec<Uuid> = Vec::with_capacity(ids.len());
    for id in ids {
        let Some((_, name, color, user_id)) = rows.iter().find(|r| r.0 == *id) else {
            continue;
        };
        let mapped = if *user_id == owner {
            *id
        } else {
            // The no-op update makes RETURNING yield the existing row on a
            // name clash, and keeps the owner's own name and color.
            sqlx::query_scalar(
                "INSERT INTO labels (name, color, user_id) VALUES ($1, $2, $3) \
                 ON CONFLICT (user_id, lower(name)) DO UPDATE SET name = labels.name \
                 RETURNING id",
            )
            .bind(name)
            .bind(color)
            .bind(owner)
            .fetch_one(&mut *conn)
            .await?
        };
        if !out.contains(&mapped) {
            out.push(mapped);
        }
    }
    Ok(out)
}

/// Attach labels to a session in its owner's vocabulary (see
/// [`owner_label_ids`]). Returns how many attachments were new. A legacy row
/// with no owner at all takes the existing labels as they are.
pub async fn attach_to_session(
    conn: &mut PgConnection,
    session_id: &str,
    ids: &[Uuid],
) -> Result<u64, sqlx::Error> {
    let mapped = match session_owner(conn, session_id).await? {
        Some(owner) => owner_label_ids(conn, owner, ids).await?,
        None => visible_label_ids(conn, ids, None).await?,
    };
    if mapped.is_empty() {
        return Ok(0);
    }
    let res = sqlx::query(
        "INSERT INTO session_labels (session_id, label_id) SELECT $1, unnest($2::uuid[]) \
         ON CONFLICT DO NOTHING",
    )
    .bind(session_id)
    .bind(&mapped)
    .execute(&mut *conn)
    .await?;
    Ok(res.rows_affected())
}

/// The labels a spawn carries, as the owner of the target machine will hold
/// them: the requested ids the caller may use (`visible_to`, `None` for an
/// admin), mapped onto `owner`'s vocabulary. Best-effort, like every label
/// path: a lookup failure spawns without labels rather than refusing.
pub async fn for_spawn(
    pool: &PgPool,
    label_ids: &[String],
    visible_to: Option<Uuid>,
    owner: Uuid,
) -> Vec<String> {
    let requested = parse_label_ids(label_ids);
    if requested.is_empty() {
        return Vec::new();
    }
    let res = async {
        let mut tx = pool.begin().await?;
        let visible = visible_label_ids(&mut tx, &requested, visible_to).await?;
        let mapped = owner_label_ids(&mut tx, owner, &visible).await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>((visible.len(), mapped))
    }
    .await;
    match res {
        Ok((visible, mapped)) => {
            if visible < requested.len() {
                tracing::warn!(
                    requested = requested.len(),
                    kept = visible,
                    "spawn labels skipped: unknown or not the caller's"
                );
            }
            mapped.iter().map(ToString::to_string).collect()
        }
        Err(e) => {
            tracing::warn!(error = %e, "could not resolve the spawn labels");
            Vec::new()
        }
    }
}

/// Remember the labels the session spawned under `spawn_key` should carry.
/// Best-effort: a lost intent leaves the session unlabelled.
pub async fn remember_intent(pool: &PgPool, spawn_key: &str, label_ids: &[String]) {
    let ids = parse_label_ids(label_ids);
    if ids.is_empty() {
        return;
    }
    if let Err(e) = sqlx::query(
        "INSERT INTO session_label_intents (spawn_key, label_ids) VALUES ($1, $2) \
         ON CONFLICT (spawn_key) DO UPDATE SET label_ids = EXCLUDED.label_ids",
    )
    .bind(spawn_key)
    .bind(&ids)
    .execute(pool)
    .await
    {
        tracing::warn!(%spawn_key, error = %e, "could not record the spawn labels");
    }
}

/// Attach the labels a pending intent holds to the session that just
/// registered. The session is looked up by its own id and by the spawn key the
/// adapter echoed, so both pre-minted (claude-code) and self-minted (codex)
/// ids resolve.
pub async fn claim_intent(pool: &PgPool, session_id: &str, spawn_key: Option<&str>) {
    let key = spawn_key.unwrap_or(session_id);
    let ids = match sqlx::query_scalar::<_, Vec<Uuid>>(
        "DELETE FROM session_label_intents WHERE spawn_key = $1 OR spawn_key = $2 \
         RETURNING label_ids",
    )
    .bind(session_id)
    .bind(key)
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows.into_iter().flatten().collect::<Vec<_>>(),
        Err(e) => {
            tracing::warn!(%session_id, error = %e, "spawn labels lookup failed");
            return;
        }
    };
    if ids.is_empty() {
        return;
    }
    attach(pool, session_id, &ids).await;
}

/// Attach a claimed intent's labels, logging what landed.
async fn attach(pool: &PgPool, session_id: &str, ids: &[Uuid]) {
    let res = async {
        let mut tx = pool.begin().await?;
        let n = attach_to_session(&mut tx, session_id, ids).await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>(n)
    }
    .await;
    match res {
        Ok(n) => {
            tracing::info!(%session_id, requested = ids.len(), attached = n, "spawn labels attached");
        }
        Err(e) => tracing::warn!(%session_id, error = %e, "could not attach the spawn labels"),
    }
}

/// Make a draft row show the labels its payload carries, so the draft card
/// displays what the launch will attach. Same rules as a spawn: the caller's
/// visible labels, in the draft owner's vocabulary.
pub async fn sync_draft(
    pool: &PgPool,
    draft_id: &str,
    label_ids: &[String],
    visible_to: Option<Uuid>,
) {
    let requested = parse_label_ids(label_ids);
    let res = async {
        let mut tx = pool.begin().await?;
        sqlx::query("DELETE FROM session_labels WHERE session_id = $1")
            .bind(draft_id)
            .execute(&mut *tx)
            .await?;
        let ids = visible_label_ids(&mut tx, &requested, visible_to).await?;
        attach_to_session(&mut tx, draft_id, &ids).await?;
        tx.commit().await
    }
    .await;
    if let Err(e) = res {
        tracing::warn!(%draft_id, error = %e, "could not sync the draft labels");
    }
}

/// Labels currently attached to a draft row: the payload's, plus any the
/// operator put on the draft card after saving it.
pub async fn draft_label_ids(pool: &PgPool, draft_id: &str) -> Vec<String> {
    sqlx::query_scalar::<_, Uuid>("SELECT label_id FROM session_labels WHERE session_id = $1")
        .bind(draft_id)
        .fetch_all(pool)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(%draft_id, error = %e, "could not read the draft labels");
            Vec::new()
        })
        .into_iter()
        .map(|id| id.to_string())
        .collect()
}

/// Reaper tick: drop intents whose spawn never registered.
pub async fn sweep(pool: &PgPool) {
    let cutoff = chrono::Utc::now() - chrono::Duration::seconds(INTENT_TTL_SECS);
    if let Err(e) = sqlx::query("DELETE FROM session_label_intents WHERE created_at < $1")
        .bind(cutoff)
        .execute(pool)
        .await
    {
        tracing::warn!(error = %e, "spawn labels intent prune failed");
    }
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;
    use uuid::Uuid;

    use super::{
        claim_intent, draft_label_ids, for_spawn, parse_label_ids, remember_intent, sync_draft,
    };

    #[test]
    fn parse_label_ids_drops_malformed_and_duplicates() {
        let a = "0b7c6f1e-2d4a-4a8e-9f3c-1a2b3c4d5e6f";
        let b = "5f0e8d7c-6b5a-4938-8271-605f4e3d2c1b";
        let raw = vec![a.to_owned(), "not-a-uuid".to_owned(), format!(" {b} "), a.to_owned()];
        let ids: Vec<String> = parse_label_ids(&raw).iter().map(ToString::to_string).collect();
        assert_eq!(ids, vec![a.to_owned(), b.to_owned()]);
    }

    #[test]
    fn parse_label_ids_empty() {
        assert!(parse_label_ids(&[]).is_empty());
    }

    async fn test_pool(test_name: &str) -> Option<PgPool> {
        let url = crate::routes::gateway::test_db_url(test_name)?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("connect test db"),
        )
    }

    async fn label(pool: &PgPool, owner: Uuid) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO labels (name, color, user_id) VALUES ($1, '#abcdef', $2) RETURNING id",
        )
        .bind(format!("spawn-labels-{}", Uuid::new_v4()))
        .bind(owner)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// A label named `name` owned by a fresh user, as an admin's picker would
    /// offer it. Returns `(that user, label id)`.
    async fn named_label(pool: &PgPool, name: &str) -> (Uuid, Uuid) {
        let them = user(pool).await;
        let id = sqlx::query_scalar(
            "INSERT INTO labels (name, color, user_id) VALUES ($1, '#abcdef', $2) RETURNING id",
        )
        .bind(name)
        .bind(them)
        .fetch_one(pool)
        .await
        .unwrap();
        (them, id)
    }

    /// `owner`'s label of that (case-insensitive) name, if any.
    async fn owned_label(pool: &PgPool, owner: Uuid, name: &str) -> Option<Uuid> {
        sqlx::query_scalar("SELECT id FROM labels WHERE user_id = $1 AND lower(name) = lower($2)")
            .bind(owner)
            .bind(name)
            .fetch_optional(pool)
            .await
            .unwrap()
    }

    async fn user(pool: &PgPool) -> Uuid {
        let uid = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("spawn-labels-{uid}"))
            .bind(format!("hsl-{uid}"))
            .execute(pool)
            .await
            .unwrap();
        uid
    }

    async fn session(pool: &PgPool, uid: Uuid, status: &str) -> String {
        let machine = Uuid::new_v4();
        let sid = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
            .bind(machine)
            .bind(uid)
            .bind(format!("mksl-{machine}"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status) \
             VALUES ($1, $2, $2, $3, '/w', $4)",
        )
        .bind(&sid)
        .bind(machine)
        .bind(uid)
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
        sid
    }

    async fn labels_of(pool: &PgPool, sid: &str) -> Vec<Uuid> {
        let mut ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT label_id FROM session_labels WHERE session_id = $1")
                .bind(sid)
                .fetch_all(pool)
                .await
                .unwrap();
        ids.sort();
        ids
    }

    /// The draft launch path: the labels ride the spawn key and land on the
    /// session the worker registers under its own id; a label deleted in
    /// between is skipped, and the intent is consumed.
    #[tokio::test]
    async fn spawn_labels_attach_on_registration() {
        let Some(pool) = test_pool("spawn_labels_attach_on_registration").await else {
            return;
        };
        let uid = user(&pool).await;
        let (a, b, gone) =
            (label(&pool, uid).await, label(&pool, uid).await, label(&pool, uid).await);
        let command_id = Uuid::new_v4().to_string();
        remember_intent(
            &pool,
            &command_id,
            &[a.to_string(), gone.to_string(), "junk".to_owned(), b.to_string()],
        )
        .await;
        sqlx::query("DELETE FROM labels WHERE id = $1").bind(gone).execute(&pool).await.unwrap();

        let sid = session(&pool, uid, "active").await;
        claim_intent(&pool, &sid, Some(&command_id)).await;
        let mut want = vec![a, b];
        want.sort();
        assert_eq!(labels_of(&pool, &sid).await, want);

        let left: i64 =
            sqlx::query_scalar("SELECT count(*) FROM session_label_intents WHERE spawn_key = $1")
                .bind(&command_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(left, 0, "the intent is consumed on claim");

        // Pre-minted ids (claude-code) register under the spawn key itself.
        let pre = session(&pool, uid, "active").await;
        remember_intent(&pool, &pre, &[a.to_string()]).await;
        claim_intent(&pool, &pre, None).await;
        assert_eq!(labels_of(&pool, &pre).await, vec![a]);

        // An intent recorded with another user's label (older rules, or an
        // admin's raw pick) lands as the owner's label of the same name.
        let (_, theirs) = named_label(&pool, "Ops").await;
        let other = session(&pool, uid, "active").await;
        remember_intent(&pool, &other, &[theirs.to_string(), a.to_string()]).await;
        claim_intent(&pool, &other, None).await;
        let mine = owned_label(&pool, uid, "ops").await.expect("owner copy created");
        let mut want = vec![a, mine];
        want.sort();
        assert_eq!(labels_of(&pool, &other).await, want);
        assert_ne!(mine, theirs);
    }

    /// The spawn-time rule: a non-admin only spawns with its own labels; an
    /// admin's pick of any user's label maps onto the machine owner's label of
    /// the same name, reused when it exists and created otherwise.
    #[tokio::test]
    async fn spawn_labels_for_spawn_maps_onto_the_owner() {
        let Some(pool) = test_pool("spawn_labels_for_spawn_maps_onto_the_owner").await else {
            return;
        };
        let owner = user(&pool).await;
        let own = label(&pool, owner).await;
        let (_, admin_new) = named_label(&pool, "Dorsk").await;
        let (_, admin_dup) = named_label(&pool, "Infra").await;
        let existing: Uuid = sqlx::query_scalar(
            "INSERT INTO labels (name, color, user_id) VALUES ('infra', '1', $1) RETURNING id",
        )
        .bind(owner)
        .fetch_one(&pool)
        .await
        .unwrap();
        let raw = [own.to_string(), admin_new.to_string(), admin_dup.to_string()];

        // A non-admin caller cannot smuggle another user's label in.
        assert_eq!(for_spawn(&pool, &raw, Some(owner), owner).await, vec![own.to_string()]);

        // The admin (god-view) keeps all three, in the owner's vocabulary.
        let got = for_spawn(&pool, &raw, None, owner).await;
        let created = owned_label(&pool, owner, "dorsk").await.expect("created for the owner");
        assert_eq!(got, vec![own.to_string(), created.to_string(), existing.to_string()]);
        let (name, color): (String, String) =
            sqlx::query_as("SELECT name, color FROM labels WHERE id = $1")
                .bind(created)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            (name.as_str(), color.as_str()),
            ("Dorsk", "#abcdef"),
            "name and color carry over"
        );
        let (name, color): (String, String) =
            sqlx::query_as("SELECT name, color FROM labels WHERE id = $1")
                .bind(existing)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            (name.as_str(), color.as_str()),
            ("infra", "1"),
            "the owner's label is kept as is"
        );

        // Mapping twice reuses the copy instead of minting another.
        assert_eq!(for_spawn(&pool, &raw, None, owner).await, got);
    }

    /// A draft row mirrors its payload's labels, and every save replaces them.
    #[tokio::test]
    async fn spawn_labels_sync_draft_row() {
        let Some(pool) = test_pool("spawn_labels_sync_draft_row").await else {
            return;
        };
        let uid = user(&pool).await;
        let (a, b) = (label(&pool, uid).await, label(&pool, uid).await);
        let draft = session(&pool, uid, "draft").await;

        sync_draft(&pool, &draft, &[a.to_string(), b.to_string()], Some(uid)).await;
        let mut want = vec![a, b];
        want.sort();
        assert_eq!(labels_of(&pool, &draft).await, want);

        sync_draft(&pool, &draft, &[b.to_string()], Some(uid)).await;
        assert_eq!(draft_label_ids(&pool, &draft).await, vec![b.to_string()]);

        sync_draft(&pool, &draft, &[], Some(uid)).await;
        assert!(labels_of(&pool, &draft).await.is_empty());

        // Another user's label: dropped for a non-admin, mapped onto the
        // draft owner's label for an admin.
        let (_, theirs) = named_label(&pool, "Recette").await;
        sync_draft(&pool, &draft, &[theirs.to_string()], Some(uid)).await;
        assert!(labels_of(&pool, &draft).await.is_empty());
        sync_draft(&pool, &draft, &[theirs.to_string()], None).await;
        let mine = owned_label(&pool, uid, "recette").await.expect("owner copy created");
        assert_eq!(labels_of(&pool, &draft).await, vec![mine]);
    }
}
