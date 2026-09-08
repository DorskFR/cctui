//! Codex model catalogs.
//!
//! Model availability is an **account** entitlement, so the source of truth is
//! the `ChatGPT` backend's models endpoint, read server-side with the account's
//! own OAuth credential (same treatment as `gateway::usage`). Catalogs reported
//! by daemons over `model/list` are kept as a per-machine fallback for accounts
//! we hold no OAuth for; on a gateway-only machine codex answers from its
//! compiled-in list, so a machine catalog never outranks an account one and
//! never replaces a stored catalog with a strict subset of itself.
//!
//! `GET /machines/{machine_id}/codex-models` returns one machine's catalog
//! (empty `models` when none is known, and the webui falls back to its static
//! offline list). `GET /models/codex` merges every catalog for pickers with no
//! machine in hand (dispatch, fork): a union by model id, account catalogs
//! first, then the newest machine report.
//! `POST /machines/{machine_id}/codex-models/refresh` re-reads every `OpenAI`
//! account's catalog from upstream — no daemon, no local `codex` binary.
//! Catalogs persist in `codex_model_catalogs` / `codex_account_model_catalogs`,
//! warmed into `AppState` on boot. Machine ownership is enforced by the
//! `authz_layer` guard (same as `fs::list_dirs`).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use cctui_proto::codex_catalog::{CodexModel, CodexModelCatalog};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::error::AppError;
use crate::routes::gateway::{current_access_token, reload_account};
use crate::state::AppState;

/// How long an account catalog is served before the next read refreshes it.
pub const ACCOUNT_CATALOG_TTL: chrono::Duration = chrono::Duration::hours(6);

/// The `ChatGPT` backend's codex model catalog. Cookieless: the account's OAuth
/// Bearer plus its `chatgpt-account-id`, exactly like `wham/usage`.
/// Overridable via env to track upstream moves.
pub fn openai_models_url() -> String {
    std::env::var("CCTUI_OPENAI_MODELS_URL")
        .unwrap_or_else(|_| "https://chatgpt.com/backend-api/codex/models".into())
}

/// The `client_version` the models endpoint is tagged with — it gates which
/// models a caller is offered, so it tracks the codex release we speak.
pub fn codex_client_version() -> String {
    std::env::var("CCTUI_CODEX_CLIENT_VERSION").unwrap_or_else(|_| "0.144.1".into())
}

#[derive(Debug, Clone)]
pub struct CachedCatalog {
    pub catalog: CodexModelCatalog,
    pub fetched_at: DateTime<Utc>,
    /// Read from upstream with the account's own credential. Outranks a
    /// machine catalog of any age in the merge.
    pub authoritative: bool,
}

/// The merged cross-machine view: model ids from every machine, each taken
/// from the most recently fetched catalog that lists it.
#[derive(Debug, Default, Serialize)]
pub struct MergedCodexCatalog {
    pub models: Vec<CodexModel>,
    pub fetched_at: Option<DateTime<Utc>>,
    pub machines: usize,
}

pub fn merge_catalogs<'a>(
    catalogs: impl IntoIterator<Item = &'a CachedCatalog>,
) -> MergedCodexCatalog {
    let mut sorted: Vec<&CachedCatalog> = catalogs.into_iter().collect();
    sorted.sort_by_key(|c| (std::cmp::Reverse(c.authoritative), std::cmp::Reverse(c.fetched_at)));
    let mut merged = MergedCodexCatalog { machines: sorted.len(), ..Default::default() };
    let mut seen = std::collections::HashSet::new();
    for cached in sorted {
        merged.fetched_at.get_or_insert(cached.fetched_at);
        for model in &cached.catalog.models {
            if seen.insert(model.id.clone()) {
                merged.models.push(model.clone());
            }
        }
    }
    merged
}

/// Whether `incoming` would degrade `existing`: it lists nothing the stored
/// catalog does not already have, and fewer models. A machine whose codex fell
/// back to its bundled list reports exactly that, and a persisted downgrade is
/// unfixable from the UI — so it is dropped instead.
pub fn is_downgrade(existing: &CodexModelCatalog, incoming: &CodexModelCatalog) -> bool {
    if incoming.models.len() >= existing.models.len() {
        return false;
    }
    let have: std::collections::HashSet<&str> =
        existing.models.iter().map(|m| m.id.as_str()).collect();
    incoming.models.iter().all(|m| have.contains(m.id.as_str()))
}

pub async fn store_catalog(state: &AppState, machine_id: Uuid, catalog: CodexModelCatalog) {
    if let Some(existing) = state.codex_catalogs.get(&machine_id)
        && is_downgrade(&existing.catalog, &catalog)
    {
        tracing::info!(%machine_id, "ignoring codex catalog report: strict subset of the stored one");
        return;
    }
    let fetched_at = Utc::now();
    let json = serde_json::to_value(&catalog).unwrap_or(serde_json::Value::Null);
    state
        .codex_catalogs
        .insert(machine_id, CachedCatalog { catalog, fetched_at, authoritative: false });
    if let Err(err) = sqlx::query(
        "INSERT INTO codex_model_catalogs (machine_id, catalog, fetched_at) VALUES ($1, $2, $3) \
         ON CONFLICT (machine_id) DO UPDATE SET catalog = EXCLUDED.catalog, fetched_at = EXCLUDED.fetched_at",
    )
    .bind(machine_id)
    .bind(json)
    .bind(fetched_at)
    .execute(&state.pool)
    .await
    {
        tracing::warn!(%machine_id, %err, "failed to persist codex model catalog");
    }
}

/// Parse the models endpoint body into a catalog. The REST shape is `snake_case`
/// where the app-server protocol is camelCase, and either `models` or `data`
/// may carry the array, so both are accepted.
pub fn parse_remote_catalog(body: &Value) -> CodexModelCatalog {
    let array = body
        .get("models")
        .or_else(|| body.get("data"))
        .and_then(Value::as_array)
        .or_else(|| body.as_array());
    let models =
        array.map(|arr| arr.iter().filter_map(parse_remote_model).collect()).unwrap_or_default();
    CodexModelCatalog { models }
}

fn field<'a>(v: &'a Value, snake: &str, camel: &str) -> Option<&'a Value> {
    v.get(snake).or_else(|| v.get(camel))
}

fn parse_remote_model(v: &Value) -> Option<CodexModel> {
    let id = field(v, "id", "slug").and_then(Value::as_str).filter(|s| !s.is_empty())?.to_owned();
    let model = field(v, "model", "modelSlug").and_then(Value::as_str).unwrap_or(&id).to_owned();
    let strings = |v: Option<&Value>| -> Vec<String> {
        v.and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| {
                        e.as_str()
                            .or_else(|| e.get("reasoning_effort").and_then(Value::as_str))
                            .or_else(|| e.get("reasoningEffort").and_then(Value::as_str))
                    })
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    Some(CodexModel {
        display_name: field(v, "display_name", "displayName")
            .and_then(Value::as_str)
            .unwrap_or(&id)
            .to_owned(),
        description: v.get("description").and_then(Value::as_str).unwrap_or_default().to_owned(),
        hidden: v.get("hidden").and_then(Value::as_bool).unwrap_or(false),
        is_default: field(v, "is_default", "isDefault").and_then(Value::as_bool).unwrap_or(false),
        default_effort: field(v, "default_reasoning_effort", "defaultReasoningEffort")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        supported_efforts: strings(field(
            v,
            "supported_reasoning_efforts",
            "supportedReasoningEfforts",
        )),
        input_modalities: strings(field(v, "input_modalities", "inputModalities")),
        upgrade: v.get("upgrade").and_then(Value::as_str).map(str::to_owned),
        id,
        model,
    })
}

/// Read one `OpenAI` account's catalog from upstream with its stored OAuth.
/// `None` when the credential can't produce a catalog (not an openai provider,
/// no `chatgpt-account-id`, refresh failure, upstream error, empty body) — the
/// caller keeps whatever it had.
pub async fn fetch_account_catalog(
    state: &AppState,
    provider_id: Uuid,
) -> Option<CodexModelCatalog> {
    let acct = reload_account(state, provider_id).await?;
    if acct.provider != "openai" {
        return None;
    }
    let account_id = acct.provider_account_id.as_deref()?;
    let access_token = current_access_token(state, &acct).await.ok()?;
    let resp = state
        .http_client
        .get(openai_models_url())
        .query(&[("client_version", codex_client_version())])
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {access_token}"))
        .header("chatgpt-account-id", account_id)
        .header(reqwest::header::ACCEPT, "*/*")
        .send()
        .await
        .map_err(|e| tracing::warn!(account = %provider_id, "codex models transport error: {e}"))
        .ok()?;
    if !resp.status().is_success() {
        tracing::warn!(account = %provider_id, status = %resp.status(), "codex models rejected");
        return None;
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| tracing::warn!(account = %provider_id, "codex models decode error: {e}"))
        .ok()?;
    let catalog = parse_remote_catalog(&body);
    if catalog.models.is_empty() { None } else { Some(catalog) }
}

pub async fn store_account_catalog(
    state: &AppState,
    provider_id: Uuid,
    catalog: CodexModelCatalog,
) {
    let fetched_at = Utc::now();
    let json = serde_json::to_value(&catalog).unwrap_or(Value::Null);
    state
        .codex_account_catalogs
        .insert(provider_id, CachedCatalog { catalog, fetched_at, authoritative: true });
    if let Err(err) = sqlx::query(
        "INSERT INTO codex_account_model_catalogs (provider_id, catalog, fetched_at) VALUES ($1, $2, $3) \
         ON CONFLICT (provider_id) DO UPDATE SET catalog = EXCLUDED.catalog, fetched_at = EXCLUDED.fetched_at",
    )
    .bind(provider_id)
    .bind(json)
    .bind(fetched_at)
    .execute(&state.pool)
    .await
    {
        tracing::warn!(account = %provider_id, %err, "failed to persist codex account catalog");
    }
}

/// Re-read every `OpenAI` account's catalog from upstream. Best-effort per
/// account: one failure never clears a stored catalog.
pub async fn refresh_account_catalogs(state: &AppState) -> usize {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM account_providers WHERE provider = 'openai' AND encrypted_refresh_token IS NOT NULL",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_else(|err| {
        tracing::warn!(%err, "failed to list openai providers for a catalog refresh");
        Vec::new()
    });
    let mut refreshed = 0;
    for id in ids {
        if let Some(catalog) = fetch_account_catalog(state, id).await {
            store_account_catalog(state, id, catalog).await;
            refreshed += 1;
        }
    }
    refreshed
}

/// Refresh account catalogs in the background when the freshest one is older
/// than [`ACCOUNT_CATALOG_TTL`], so a new upstream model appears without any
/// per-machine action or webui release.
fn refresh_account_catalogs_if_stale(state: &AppState) {
    let freshest = state.codex_account_catalogs.iter().map(|c| c.fetched_at).max();
    if freshest.is_some_and(|at| Utc::now() - at < ACCOUNT_CATALOG_TTL) {
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        refresh_account_catalogs(&state).await;
    });
}

pub async fn warm_cache(state: &AppState) {
    let rows: Vec<(Uuid, serde_json::Value, DateTime<Utc>)> =
        sqlx::query_as("SELECT provider_id, catalog, fetched_at FROM codex_account_model_catalogs")
            .fetch_all(&state.pool)
            .await
            .unwrap_or_else(|err| {
                tracing::warn!(%err, "failed to load codex account catalogs");
                Vec::new()
            });
    for (provider_id, json, fetched_at) in rows {
        match serde_json::from_value::<CodexModelCatalog>(json) {
            Ok(catalog) => {
                state.codex_account_catalogs.insert(
                    provider_id,
                    CachedCatalog { catalog, fetched_at, authoritative: true },
                );
            }
            Err(err) => {
                tracing::warn!(account = %provider_id, %err, "malformed persisted codex catalog");
            }
        }
    }
    warm_machine_cache(state).await;
}

async fn warm_machine_cache(state: &AppState) {
    let rows: Vec<(Uuid, serde_json::Value, DateTime<Utc>)> =
        match sqlx::query_as("SELECT machine_id, catalog, fetched_at FROM codex_model_catalogs")
            .fetch_all(&state.pool)
            .await
        {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!(%err, "failed to load codex model catalogs");
                return;
            }
        };
    for (machine_id, json, fetched_at) in rows {
        match serde_json::from_value::<CodexModelCatalog>(json) {
            Ok(catalog) => {
                state.codex_catalogs.insert(
                    machine_id,
                    CachedCatalog { catalog, fetched_at, authoritative: false },
                );
            }
            Err(err) => tracing::warn!(%machine_id, %err, "malformed persisted codex catalog"),
        }
    }
}

fn parse_machine(machine_id: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(machine_id)
        .map_err(|_| AppError::new(StatusCode::BAD_REQUEST, "machine_id must be a uuid"))
}

pub async fn get_codex_models(
    State(state): State<AppState>,
    Path(machine_id): Path<String>,
) -> Result<Json<CodexModelCatalog>, AppError> {
    let machine_uuid = parse_machine(&machine_id)?;
    refresh_account_catalogs_if_stale(&state);
    let accounts: Vec<CachedCatalog> =
        state.codex_account_catalogs.iter().map(|c| c.value().clone()).collect();
    if !accounts.is_empty() {
        return Ok(Json(CodexModelCatalog { models: merge_catalogs(&accounts).models }));
    }
    let catalog =
        state.codex_catalogs.get(&machine_uuid).map(|c| c.catalog.clone()).unwrap_or_default();
    Ok(Json(catalog))
}

pub async fn get_merged_codex_models(
    State(state): State<AppState>,
) -> Result<Json<MergedCodexCatalog>, AppError> {
    refresh_account_catalogs_if_stale(&state);
    let cached: Vec<CachedCatalog> = state
        .codex_account_catalogs
        .iter()
        .chain(state.codex_catalogs.iter())
        .map(|c| c.value().clone())
        .collect();
    Ok(Json(merge_catalogs(&cached)))
}

/// The ↻ button. The machine is only the caller's authorization handle: the
/// catalog is read from upstream per account, since a gateway-only machine's
/// codex can only answer from its compiled-in list.
pub async fn refresh_codex_models(
    State(state): State<AppState>,
    Path(machine_id): Path<String>,
) -> Result<StatusCode, AppError> {
    parse_machine(&machine_id)?;
    refresh_account_catalogs(&state).await;
    Ok(StatusCode::ACCEPTED)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, display_name: &str) -> CodexModel {
        CodexModel {
            id: id.to_owned(),
            model: id.to_owned(),
            display_name: display_name.to_owned(),
            description: String::new(),
            hidden: false,
            is_default: false,
            supported_efforts: vec![],
            default_effort: String::new(),
            input_modalities: vec![],
            upgrade: None,
        }
    }

    fn cached(models: Vec<CodexModel>, secs: i64) -> CachedCatalog {
        CachedCatalog {
            catalog: CodexModelCatalog { models },
            fetched_at: DateTime::from_timestamp(secs, 0).unwrap(),
            authoritative: false,
        }
    }

    #[test]
    fn merge_is_a_union_where_the_newest_report_wins() {
        let old = cached(vec![model("gpt-a", "A old"), model("gpt-old-only", "Old only")], 10);
        let new = cached(vec![model("gpt-a", "A new"), model("gpt-b", "B")], 20);
        let merged = merge_catalogs([&old, &new]);
        let labels: Vec<(&str, &str)> =
            merged.models.iter().map(|m| (m.id.as_str(), m.display_name.as_str())).collect();
        assert_eq!(labels, [("gpt-a", "A new"), ("gpt-b", "B"), ("gpt-old-only", "Old only")]);
        assert_eq!(merged.fetched_at, Some(new.fetched_at));
        assert_eq!(merged.machines, 2);
    }

    #[test]
    fn an_account_catalog_outranks_a_newer_machine_catalog() {
        let machine = cached(vec![model("gpt-5.5", "GPT-5.5 machine")], 99);
        let account = CachedCatalog {
            authoritative: true,
            ..cached(vec![model("gpt-6-astra", "Astra"), model("gpt-5.5", "GPT-5.5")], 1)
        };
        let merged = merge_catalogs([&machine, &account]);
        let ids: Vec<&str> = merged.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["gpt-6-astra", "gpt-5.5"]);
        assert_eq!(merged.models[1].display_name, "GPT-5.5");
    }

    #[test]
    fn a_bundled_fallback_report_never_replaces_a_richer_catalog() {
        let stored = CodexModelCatalog {
            models: vec![model("gpt-6-astra", "Astra"), model("gpt-5.5", "GPT-5.5")],
        };
        let fallback = CodexModelCatalog { models: vec![model("gpt-5.5", "GPT-5.5")] };
        assert!(is_downgrade(&stored, &fallback));
        assert!(!is_downgrade(&fallback, &stored));
        assert!(!is_downgrade(&stored, &stored.clone()));
        let sideways = CodexModelCatalog { models: vec![model("gpt-7", "New")] };
        assert!(!is_downgrade(&stored, &sideways));
    }

    #[test]
    fn the_models_endpoint_body_parses_in_either_casing() {
        let body = serde_json::json!({"models": [{
            "id": "gpt-6-astra",
            "display_name": "GPT-6-Astra",
            "is_default": true,
            "supported_reasoning_efforts": ["low", "high"],
            "default_reasoning_effort": "high"
        }, {
            "id": "gpt-5.5",
            "displayName": "GPT-5.5",
            "supportedReasoningEfforts": [{"reasoningEffort": "medium"}]
        }, {
            "displayName": "no id, dropped"
        }]});
        let catalog = parse_remote_catalog(&body);
        let ids: Vec<&str> = catalog.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["gpt-6-astra", "gpt-5.5"]);
        assert_eq!(catalog.models[0].display_name, "GPT-6-Astra");
        assert_eq!(catalog.models[0].supported_efforts, ["low", "high"]);
        assert!(catalog.models[0].is_default);
        assert_eq!(catalog.models[1].display_name, "GPT-5.5");
        assert_eq!(catalog.models[1].supported_efforts, ["medium"]);
        assert_eq!(catalog.models[1].model, "gpt-5.5");
    }

    #[test]
    fn a_body_without_models_parses_to_an_empty_catalog() {
        assert!(parse_remote_catalog(&serde_json::json!({"error": "nope"})).models.is_empty());
        assert_eq!(parse_remote_catalog(&serde_json::json!([{"id": "a"}])).models.len(), 1);
    }

    #[test]
    fn merge_of_nothing_is_empty() {
        let merged = merge_catalogs([]);
        assert!(merged.models.is_empty());
        assert_eq!(merged.fetched_at, None);
        assert_eq!(merged.machines, 0);
    }
}
