//! The machines and labels a filter can name.
//!
//! [`super::filter_matches`] compares `match.machines` and `match.labels`
//! against ids, so a save resolves names to the owner's ids and rejects
//! anything else. Only an all-users item may name another user's machine.

use axum::http::StatusCode;
use sqlx::PgPool;
use uuid::Uuid;

use super::{ContextItem, ContextItemSpec, ContextMatch, ContextReach, scope_of};
use crate::error::AppError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub id: String,
    /// The name shown first; every name resolves.
    pub names: Vec<String>,
}

impl Target {
    fn shown(&self) -> String {
        format!("{} ({})", self.names.first().map_or("", String::as_str), short(&self.id))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Targets {
    pub machines: Vec<Target>,
    pub labels: Vec<Target>,
}

fn short(id: &str) -> String {
    match id.get(..8) {
        Some(head) if id.len() > 8 => format!("{head}…"),
        _ => id.to_owned(),
    }
}

fn listing<'a>(targets: impl IntoIterator<Item = &'a Target>) -> String {
    targets.into_iter().map(Target::shown).collect::<Vec<_>>().join(", ")
}

fn resolve_one(
    what: &str,
    value: &str,
    known: &[Target],
    keep: &[&str],
) -> Result<String, AppError> {
    if keep.contains(&value) {
        return Ok(value.to_owned());
    }
    if let Ok(id) = Uuid::parse_str(value) {
        let id = id.to_string();
        if known.iter().any(|t| t.id == id) {
            return Ok(id);
        }
    }
    let hits: Vec<&Target> =
        known.iter().filter(|t| t.names.iter().any(|n| n.eq_ignore_ascii_case(value))).collect();
    let bad = |msg: String| AppError::new(StatusCode::BAD_REQUEST, msg);
    match hits.as_slice() {
        [one] => Ok(one.id.clone()),
        [] if known.is_empty() => {
            Err(bad(format!("unknown {what} `{value}`: you have no {what}s")))
        }
        [] => Err(bad(format!(
            "unknown {what} `{value}`: use the id or name of one of your {what}s: {}",
            listing(known)
        ))),
        many => Err(bad(format!(
            "{what} name `{value}` is ambiguous, it matches {}: use the id",
            listing(many.iter().copied())
        ))),
    }
}

fn resolve_all(
    what: &str,
    values: &[String],
    known: &[Target],
    keep: &[&str],
) -> Result<Vec<String>, AppError> {
    let mut out =
        values.iter().map(|v| resolve_one(what, v, known, keep)).collect::<Result<Vec<_>, _>>()?;
    out.sort();
    out.dedup();
    Ok(out)
}

/// The filters an edit of `item` may carry over unchanged, so a machine
/// deleted since does not block editing the body.
#[must_use]
pub fn kept(item: &ContextItem) -> Vec<&ContextMatch> {
    [Some(&item.match_), item.pending.as_ref().map(|p| &p.match_)].into_iter().flatten().collect()
}

const fn names_targets(m: &ContextMatch) -> bool {
    !m.machines.is_empty() || !m.labels.is_empty()
}

impl Targets {
    pub async fn load(pool: &PgPool, owner: Uuid) -> Result<Self, sqlx::Error> {
        Self::load_with(pool, owner, false).await
    }

    /// With `every_machine`, the machines of every user, for an all-users item.
    pub async fn load_with(
        pool: &PgPool,
        owner: Uuid,
        every_machine: bool,
    ) -> Result<Self, sqlx::Error> {
        let machines: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
            "SELECT id, name, display_name FROM machines \
             WHERE ($2 OR user_id = $1) AND deleted_at IS NULL \
             ORDER BY lower(coalesce(display_name, name)), id",
        )
        .bind(owner)
        .bind(every_machine)
        .fetch_all(pool)
        .await?;
        let labels: Vec<(Uuid, String)> =
            sqlx::query_as("SELECT id, name FROM labels WHERE user_id = $1 ORDER BY lower(name)")
                .bind(owner)
                .fetch_all(pool)
                .await?;
        let machine = |(id, name, display): (Uuid, String, Option<String>)| Target {
            id: id.to_string(),
            names: display
                .filter(|d| !d.trim().is_empty() && *d != name)
                .into_iter()
                .chain([name])
                .collect(),
        };
        Ok(Self {
            machines: machines.into_iter().map(machine).collect(),
            labels: labels
                .into_iter()
                .map(|(id, name)| Target { id: id.to_string(), names: vec![name] })
                .collect(),
        })
    }

    pub fn resolve(
        &self,
        m: ContextMatch,
        keep: &[&ContextMatch],
    ) -> Result<ContextMatch, AppError> {
        let kept_machines: Vec<&str> =
            keep.iter().flat_map(|k| k.machines.iter().map(String::as_str)).collect();
        let kept_labels: Vec<&str> =
            keep.iter().flat_map(|k| k.labels.iter().map(String::as_str)).collect();
        Ok(ContextMatch {
            machines: resolve_all("machine", &m.machines, &self.machines, &kept_machines)?,
            labels: resolve_all("label", &m.labels, &self.labels, &kept_labels)?,
            ..m
        })
    }

    pub fn spec(
        &self,
        mut spec: ContextItemSpec,
        keep: &[&ContextMatch],
    ) -> Result<ContextItemSpec, AppError> {
        if !names_targets(&spec.match_) {
            return Ok(spec);
        }
        spec.match_ = self.resolve(std::mem::take(&mut spec.match_), keep)?;
        (spec.scope, spec.scope_ref) = scope_of(&spec.match_);
        Ok(spec)
    }

    #[must_use]
    pub fn render(&self, m: &ContextMatch) -> String {
        let reach = match m.reach {
            ContextReach::Owner => "my machines",
            ContextReach::AllUsers => "all users' machines (admin)",
        };
        let mut out = reach.to_owned();
        if m.dispatch {
            out.push_str(" + my dispatches");
        }
        let keys = self.render_keys(m);
        if !keys.is_empty() {
            out.push_str(": ");
            out.push_str(&keys);
        }
        out
    }

    fn render_keys(&self, m: &ContextMatch) -> String {
        let named = |ids: &[String], known: &[Target]| {
            ids.iter()
                .map(|id| {
                    known
                        .iter()
                        .find(|t| t.id == *id)
                        .map_or_else(|| format!("unknown ({})", short(id)), Target::shown)
                })
                .collect::<Vec<_>>()
                .join(" | ")
        };
        let mut parts = Vec::new();
        if !m.machines.is_empty() {
            parts.push(format!("machines={}", named(&m.machines, &self.machines)));
        }
        if !m.paths.is_empty() {
            parts.push(format!("paths={}", m.paths.join(" | ")));
        }
        if !m.labels.is_empty() {
            parts.push(format!("labels={}", named(&m.labels, &self.labels)));
        }
        if !m.adapters.is_empty() {
            parts.push(format!("adapters={}", m.adapters.join(" | ")));
        }
        if !m.dispatchers.is_empty() {
            parts.push(format!("dispatchers={}", m.dispatchers.join(" | ")));
        }
        if !m.profiles.is_empty() {
            parts.push(format!("profiles={}", m.profiles.join(" | ")));
        }
        parts.join(", ")
    }
}

/// 422 when `m` names, by id, a machine of a user other than `owner`, unless
/// the item reaches all users' machines.
pub async fn refuse_foreign_machines(
    pool: &PgPool,
    owner: Uuid,
    m: &ContextMatch,
) -> Result<(), AppError> {
    if m.reach == ContextReach::AllUsers {
        return Ok(());
    }
    let ids: Vec<Uuid> = m.machines.iter().filter_map(|v| Uuid::parse_str(v).ok()).collect();
    if ids.is_empty() {
        return Ok(());
    }
    let foreign: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM machines WHERE id = ANY($1) AND user_id <> $2 ORDER BY id",
    )
    .bind(&ids)
    .bind(owner)
    .fetch_all(pool)
    .await?;
    if foreign.is_empty() {
        return Ok(());
    }
    let listed = foreign.iter().map(|id| short(&id.to_string())).collect::<Vec<_>>().join(", ");
    Err(AppError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        format!(
            "machine {listed} belongs to another user: an item reaches only its owner's \
             machines unless an admin sets it to reach all users' machines"
        ),
    ))
}

pub async fn resolve_spec(
    pool: &PgPool,
    owner: Uuid,
    spec: ContextItemSpec,
    keep: &[&ContextMatch],
) -> Result<ContextItemSpec, AppError> {
    if !names_targets(&spec.match_) {
        return Ok(spec);
    }
    refuse_foreign_machines(pool, owner, &spec.match_).await?;
    let every_machine = spec.match_.reach == ContextReach::AllUsers;
    Targets::load_with(pool, owner, every_machine).await?.spec(spec, keep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::context::{clean_spec, get_one, insert};

    const DEV1: &str = "3d3d04a3-0000-4000-8000-000000000001";
    const BOX_A: &str = "aaaaaaaa-0000-4000-8000-000000000002";
    const BOX_B: &str = "bbbbbbbb-0000-4000-8000-000000000003";
    const OPS: &str = "cccccccc-0000-4000-8000-000000000004";

    fn target(id: &str, names: &[&str]) -> Target {
        Target { id: id.to_owned(), names: names.iter().map(|n| (*n).to_owned()).collect() }
    }

    fn fleet() -> Targets {
        Targets {
            machines: vec![
                target(DEV1, &["dev1"]),
                target(BOX_A, &["devbox"]),
                target(BOX_B, &["Laptop", "devbox"]),
            ],
            labels: vec![target(OPS, &["ops"])],
        }
    }

    fn machines(values: &[&str]) -> ContextMatch {
        ContextMatch {
            machines: values.iter().map(|v| (*v).to_owned()).collect(),
            ..ContextMatch::default()
        }
    }

    fn message(err: &AppError) -> String {
        err.to_string()
    }

    #[test]
    fn a_unique_machine_name_resolves_to_its_id() {
        let m = fleet().resolve(machines(&["dev1"]), &[]).expect("resolves");
        assert_eq!(m.machines, [DEV1]);
        let m = fleet().resolve(machines(&["laptop"]), &[]).expect("a display name resolves");
        assert_eq!(m.machines, [BOX_B]);
    }

    #[test]
    fn an_owned_machine_id_is_accepted_and_normalized() {
        let upper = DEV1.to_uppercase();
        let m = fleet().resolve(machines(&[upper.as_str(), DEV1]), &[]).expect("an id");
        assert_eq!(m.machines, [DEV1]);
    }

    #[test]
    fn an_ambiguous_machine_name_is_rejected_with_its_candidates() {
        let err = fleet().resolve(machines(&["devbox"]), &[]).expect_err("ambiguous");
        let msg = message(&err);
        assert!(msg.contains("ambiguous"), "{msg}");
        assert!(msg.contains("devbox (aaaaaaaa…)"), "{msg}");
        assert!(msg.contains("Laptop (bbbbbbbb…)"), "{msg}");
        assert!(!msg.contains("dev1"), "{msg}");
    }

    #[test]
    fn an_unknown_machine_is_rejected_naming_the_known_ones() {
        let err = fleet().resolve(machines(&["ghost"]), &[]).expect_err("unknown");
        let msg = message(&err);
        assert!(msg.contains("unknown machine `ghost`"), "{msg}");
        assert!(msg.contains("dev1 (3d3d04a3…)"), "{msg}");
        let foreign = Uuid::new_v4().to_string();
        assert!(fleet().resolve(machines(&[foreign.as_str()]), &[]).is_err(), "another user's id");
        let none = Targets::default().resolve(machines(&["dev1"]), &[]).expect_err("no machines");
        assert!(message(&none).contains("you have no machines"), "{}", message(&none));
    }

    #[test]
    fn labels_resolve_by_name_and_reject_unknown_names() {
        let m = ContextMatch { labels: vec!["OPS".into()], ..ContextMatch::default() };
        assert_eq!(fleet().resolve(m, &[]).expect("resolves").labels, [OPS]);
        let m = ContextMatch { labels: vec!["prod".into()], ..ContextMatch::default() };
        let msg = message(&fleet().resolve(m, &[]).expect_err("unknown"));
        assert!(msg.contains("unknown label `prod`") && msg.contains("ops (cccccccc…)"), "{msg}");
    }

    #[test]
    fn an_edit_keeps_a_value_already_stored_on_the_item() {
        let stored = machines(&["dead0000-0000-4000-8000-000000000009"]);
        let m = fleet().resolve(stored.clone(), &[&stored]).expect("kept");
        assert_eq!(m, stored);
    }

    #[test]
    fn a_resolved_spec_keeps_its_legacy_scope_in_step() {
        let spec = clean_spec(ContextItemSpec {
            kind: "rule".into(),
            name: "wt".into(),
            title: "W".into(),
            scope: "machine".into(),
            scope_ref: Some("dev1".into()),
            ..ContextItemSpec::default()
        })
        .expect("clean");
        let spec = fleet().spec(spec, &[]).expect("resolves");
        assert_eq!(spec.match_.machines, [DEV1]);
        assert_eq!((spec.scope.as_str(), spec.scope_ref.as_deref()), ("machine", Some(DEV1)));
    }

    #[test]
    fn a_filter_renders_names_beside_short_ids() {
        let t = fleet();
        assert_eq!(t.render(&ContextMatch::default()), "my machines");
        let m = ContextMatch {
            machines: vec![DEV1.into()],
            paths: vec!["/w/repo".into()],
            ..ContextMatch::default()
        };
        assert_eq!(t.render(&m), "my machines: machines=dev1 (3d3d04a3…), paths=/w/repo");
        let m = ContextMatch {
            machines: vec!["dev1".into(), BOX_B.into()],
            labels: vec![OPS.into()],
            adapters: vec!["codex".into()],
            ..ContextMatch::default()
        };
        assert_eq!(
            t.render(&m),
            "my machines: machines=unknown (dev1) | Laptop (bbbbbbbb…), labels=ops (cccccccc…), \
             adapters=codex"
        );
        let wide = ContextMatch {
            reach: ContextReach::AllUsers,
            dispatch: true,
            dispatchers: vec!["kube".into()],
            ..ContextMatch::default()
        };
        assert_eq!(
            t.render(&wide),
            "all users' machines (admin) + my dispatches: dispatchers=kube",
            "never a bare everywhere"
        );
    }

    async fn seed(pool: &PgPool) -> (Uuid, Uuid, Uuid) {
        let uid = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("ctx-targets-{uid}"))
            .bind(format!("hctx-targets-{uid}"))
            .execute(pool)
            .await
            .expect("seed user");
        let mut ids = Vec::new();
        for name in ["dev1", "devbox", "devbox"] {
            let mid = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)",
            )
            .bind(mid)
            .bind(uid)
            .bind(name)
            .bind(format!("mk-{mid}"))
            .execute(pool)
            .await
            .expect("seed machine");
            ids.push(mid);
        }
        (uid, ids[0], ids[1])
    }

    async fn cleanup(pool: &PgPool, uid: Uuid) {
        sqlx::query("DELETE FROM context_items WHERE user_id = $1")
            .bind(uid)
            .execute(pool)
            .await
            .ok();
        sqlx::query("DELETE FROM machines WHERE user_id = $1").bind(uid).execute(pool).await.ok();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(pool).await.ok();
    }

    fn rule(m: ContextMatch) -> ContextItemSpec {
        clean_spec(ContextItemSpec {
            kind: "rule".into(),
            name: "scoped".into(),
            title: "Scoped".into(),
            body: "b".into(),
            match_: m,
            ..ContextItemSpec::default()
        })
        .expect("clean")
    }

    #[tokio::test]
    async fn saving_a_machine_name_stores_its_id_and_renders_it() {
        let Some(url) = cctui_server_kernel::test_support::db_url("context_targets") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (uid, dev1, devbox) = seed(&pool).await;

        let spec = resolve_spec(&pool, uid, rule(machines(&["dev1"])), &[]).await.expect("unique");
        let saved = insert(&pool, uid, &spec, None, None).await.expect("insert");
        let stored = get_one(&pool, uid, saved.id).await.expect("lookup").expect("there");
        assert_eq!(stored.match_.machines, [dev1.to_string()], "never the raw name");
        assert_eq!(stored.scope_ref.as_deref(), Some(dev1.to_string().as_str()));

        let devbox_id = devbox.to_string();
        let by_id = resolve_spec(&pool, uid, rule(machines(&[devbox_id.as_str()])), &[])
            .await
            .expect("an owned id");
        assert_eq!(by_id.match_.machines, [devbox_id]);

        let err = resolve_spec(&pool, uid, rule(machines(&["devbox"])), &[])
            .await
            .expect_err("two machines are named devbox");
        let msg = message(&err);
        assert!(msg.contains("ambiguous") && msg.contains(&devbox.to_string()[..8]), "{msg}");

        let err = resolve_spec(&pool, uid, rule(machines(&["ghost"])), &[])
            .await
            .expect_err("no such machine");
        let msg = message(&err);
        assert!(msg.contains("unknown machine `ghost`"), "{msg}");
        assert!(msg.contains(&format!("dev1 ({}…)", &dev1.to_string()[..8])), "{msg}");

        let targets = Targets::load(&pool, uid).await.expect("load");
        let shown = targets.render(&stored.match_);
        assert_eq!(shown, format!("my machines: machines=dev1 ({}…)", &dev1.to_string()[..8]));

        cleanup(&pool, uid).await;
    }

    #[tokio::test]
    async fn another_users_machine_id_is_refused_with_422_unless_the_item_reaches_all_users() {
        let Some(url) = cctui_server_kernel::test_support::db_url("context_targets_foreign") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (uid, mine, _) = seed(&pool).await;
        let (other, theirs, _) = seed(&pool).await;
        let theirs_id = theirs.to_string();

        let err = resolve_spec(&pool, uid, rule(machines(&[theirs_id.as_str()])), &[])
            .await
            .expect_err("another user's machine");
        assert_eq!(err.status(), StatusCode::UNPROCESSABLE_ENTITY, "{}", message(&err));
        assert!(message(&err).contains(&theirs_id[..8]), "{}", message(&err));

        let mixed = machines(&[mine.to_string().as_str(), theirs_id.as_str()]);
        let kept = resolve_spec(&pool, uid, rule(mixed.clone()), &[&mixed]).await;
        assert_eq!(
            kept.expect_err("kept is no excuse").status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "an edit that keeps a foreign id is still refused"
        );

        let wide =
            ContextMatch { reach: ContextReach::AllUsers, ..machines(&[theirs_id.as_str()]) };
        let spec = resolve_spec(&pool, uid, rule(wide), &[]).await.expect("an all-users item");
        assert_eq!(spec.match_.machines, [theirs_id]);

        cleanup(&pool, uid).await;
        cleanup(&pool, other).await;
    }
}
