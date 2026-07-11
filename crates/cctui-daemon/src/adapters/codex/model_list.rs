//! Codex `model/list` catalog poll (CCT-641).
//!
//! A short-lived stdio `codex app-server` per poll (same one-shot pattern as
//! [`super::thread_list`]): spawn, `initialize` → `model/list`, read the
//! response, exit. The parsed [`CodexModelCatalog`] is machine/account-scoped —
//! it reflects exactly the models the signed-in account can use — and is
//! shipped to the server as an [`AdapterEvent::CodexModels`] event.

use std::process::Stdio;
use std::time::Duration;

use cctui_proto::adapter::AdapterEvent;
use cctui_proto::codex_catalog::{CodexModel, CodexModelCatalog};
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::app_server::AppServerConfig;

const MAX_PAGES: usize = 20;
const POLL_TIMEOUT: Duration = Duration::from_secs(30);

/// Parse one `model/list` `data[]` element (codex 0.144.1 `Model`) into a
/// [`CodexModel`]. Returns `None` for entries missing a usable id.
#[must_use]
pub fn parse_model(v: &Value) -> Option<CodexModel> {
    let id = v.get("id").and_then(Value::as_str).filter(|s| !s.is_empty())?.to_owned();
    let model = v.get("model").and_then(Value::as_str).unwrap_or(&id).to_owned();
    let supported_efforts = v
        .get("supportedReasoningEfforts")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.get("reasoningEffort").and_then(Value::as_str).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let input_modalities = v
        .get("inputModalities")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(|e| e.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    Some(CodexModel {
        display_name: v.get("displayName").and_then(Value::as_str).unwrap_or(&id).to_owned(),
        description: v.get("description").and_then(Value::as_str).unwrap_or_default().to_owned(),
        hidden: v.get("hidden").and_then(Value::as_bool).unwrap_or(false),
        is_default: v.get("isDefault").and_then(Value::as_bool).unwrap_or(false),
        default_effort: v
            .get("defaultReasoningEffort")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        supported_efforts,
        input_modalities,
        upgrade: v.get("upgrade").and_then(Value::as_str).map(str::to_owned),
        id,
        model,
    })
}

/// Parse the `result` of a `model/list` response into models.
#[must_use]
pub fn parse_model_list(result: &Value) -> Vec<CodexModel> {
    result
        .get("data")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(parse_model).collect())
        .unwrap_or_default()
}

fn initialize_req() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {"clientInfo": {"name": "cctui", "version": env!("CARGO_PKG_VERSION")}},
    })
}

fn model_list_req(id: i64, cursor: Option<&str>) -> Value {
    let mut params = serde_json::Map::new();
    params.insert("includeHidden".to_owned(), json!(true));
    if let Some(cursor) = cursor {
        params.insert("cursor".to_owned(), json!(cursor));
    }
    json!({"jsonrpc": "2.0", "id": id, "method": "model/list", "params": Value::Object(params)})
}

#[must_use]
fn next_cursor(result: &Value) -> Option<String> {
    result.get("nextCursor").and_then(Value::as_str).filter(|c| !c.is_empty()).map(str::to_owned)
}

#[derive(Debug, Clone)]
pub struct ModelListConfig {
    pub app: AppServerConfig,
    pub poll_interval: Duration,
}

impl ModelListConfig {
    pub fn from_value(v: &Value) -> Self {
        let mut cfg =
            Self { app: AppServerConfig::from_value(v), poll_interval: Duration::from_secs(300) };
        if let Some(ms) = v.get("model_catalog_poll_ms").and_then(Value::as_u64) {
            cfg.poll_interval = Duration::from_millis(ms);
        }
        cfg
    }

    /// `false` disables the catalog poll (`model_catalog = false`). Enabled by
    /// default; degrades silently to the webui's static fallback list.
    pub fn enabled(v: &Value) -> bool {
        v.get("model_catalog").and_then(Value::as_bool).unwrap_or(true)
    }
}

/// Poll `model/list` at startup and every `poll_interval`, emitting an
/// [`AdapterEvent::CodexModels`] each time the catalog is fetched. Probe
/// failures (codex missing, sandbox/userns, auth) are logged at debug and the
/// tick continues — the server keeps the last known catalog, the webui the
/// static fallback.
pub struct ModelCatalogPoll {
    cfg: ModelListConfig,
    events: mpsc::Sender<AdapterEvent>,
    shutdown: CancellationToken,
}

impl ModelCatalogPoll {
    pub const fn new(
        cfg: ModelListConfig,
        events: mpsc::Sender<AdapterEvent>,
        shutdown: CancellationToken,
    ) -> Self {
        Self { cfg, events, shutdown }
    }

    pub async fn run(self) {
        let mut tick = tokio::time::interval(self.cfg.poll_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = self.shutdown.cancelled() => return,
                _ = tick.tick() => match poll_models(&self.cfg.app).await {
                    Ok(models) => {
                        let catalog = CodexModelCatalog { models };
                        if self.events.send(AdapterEvent::CodexModels { catalog }).await.is_err() {
                            return;
                        }
                    }
                    Err(err) => tracing::debug!(%err, "codex model/list catalog poll failed"),
                },
            }
        }
    }
}

/// Spawn a short-lived stdio `codex app-server`, run initialize → `model/list`
/// (paginating on `nextCursor`), and return the parsed models. The process is
/// reaped before returning.
pub async fn poll_models(app: &AppServerConfig) -> anyhow::Result<Vec<CodexModel>> {
    let mut cmd = Command::new(&app.bin);
    cmd.arg("app-server")
        .arg("-c")
        .arg(format!("sandbox_mode=\"{}\"", app.sandbox_mode))
        .env("PATH", crate::childenv::child_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd.spawn()?;
    let mut stdin =
        child.stdin.take().ok_or_else(|| anyhow::anyhow!("app-server stdin missing"))?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("app-server stdout missing"))?;

    let models = tokio::time::timeout(POLL_TIMEOUT, async {
        let mut lines = BufReader::new(stdout).lines();
        write_line(&mut stdin, &initialize_req()).await?;
        let mut models = Vec::new();
        let mut cursor: Option<String> = None;
        for page in 0..MAX_PAGES {
            let req_id = 2 + i64::try_from(page).unwrap_or(i64::MAX);
            write_line(&mut stdin, &model_list_req(req_id, cursor.as_deref())).await?;
            let result = read_response(&mut lines, req_id).await?;
            models.extend(parse_model_list(&result));
            match next_cursor(&result) {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        anyhow::Ok(models)
    })
    .await;

    drop(stdin);
    let _ = child.start_kill();
    let _ = child.wait().await;

    models.map_err(|_| anyhow::anyhow!("model/list timed out"))?
}

async fn read_response<R: AsyncBufRead + Unpin>(
    lines: &mut Lines<R>,
    id: i64,
) -> anyhow::Result<Value> {
    while let Some(line) = lines.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(trimmed) else { continue };
        if v.get("id").and_then(Value::as_i64) == Some(id) {
            if let Some(err) = v.get("error") {
                anyhow::bail!("model/list error: {err}");
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }
    anyhow::bail!("model/list response {id} not received before EOF")
}

async fn write_line<W: AsyncWriteExt + Unpin>(w: &mut W, v: &Value) -> anyhow::Result<()> {
    let mut line = serde_json::to_string(v)?;
    line.push('\n');
    w.write_all(line.as_bytes()).await?;
    w.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        json!({"data": [
            {
                "id": "gpt-5.6-sol",
                "model": "gpt-5.6-sol",
                "displayName": "GPT-5.6 Sol",
                "description": "Flagship reasoning model.",
                "hidden": false,
                "isDefault": true,
                "defaultReasoningEffort": "medium",
                "supportedReasoningEfforts": [
                    {"reasoningEffort": "low", "description": "fast"},
                    {"reasoningEffort": "medium", "description": "balanced"},
                    {"reasoningEffort": "high", "description": "thorough"}
                ],
                "inputModalities": ["text", "image"],
                "upgrade": null
            },
            {
                "id": "gpt-5.4",
                "model": "gpt-5.4",
                "displayName": "GPT-5.4",
                "description": "",
                "hidden": true,
                "isDefault": false,
                "defaultReasoningEffort": "medium",
                "supportedReasoningEfforts": [
                    {"reasoningEffort": "low", "description": "fast"}
                ],
                "inputModalities": ["text"],
                "upgrade": "gpt-5.6-sol"
            },
            { "displayName": "no id, dropped" }
        ], "nextCursor": null})
    }

    #[test]
    fn parses_models_and_skips_idless() {
        let models = parse_model_list(&sample());
        assert_eq!(models.len(), 2);
        let sol = &models[0];
        assert_eq!(sol.id, "gpt-5.6-sol");
        assert_eq!(sol.display_name, "GPT-5.6 Sol");
        assert!(sol.is_default);
        assert!(!sol.hidden);
        assert_eq!(sol.default_effort, "medium");
        assert_eq!(sol.supported_efforts, ["low", "medium", "high"]);
        assert_eq!(sol.input_modalities, ["text", "image"]);
        assert_eq!(sol.upgrade, None);

        let old = &models[1];
        assert!(old.hidden);
        assert_eq!(old.upgrade.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(old.supported_efforts, ["low"]);
    }

    #[test]
    fn model_falls_back_to_id_when_slug_and_label_missing() {
        let m = parse_model(&json!({"id": "gpt-x"})).unwrap();
        assert_eq!(m.model, "gpt-x");
        assert_eq!(m.display_name, "gpt-x");
        assert!(m.supported_efforts.is_empty());
        assert!(m.input_modalities.is_empty());
    }

    #[test]
    fn missing_data_is_empty() {
        assert!(parse_model_list(&json!({})).is_empty());
        assert!(parse_model_list(&Value::Null).is_empty());
    }

    #[test]
    fn request_builders_shape() {
        assert_eq!(initialize_req()["method"], "initialize");
        let r = model_list_req(2, None);
        assert_eq!(r["method"], "model/list");
        assert_eq!(r["params"]["includeHidden"], true);
        assert!(r["params"].get("cursor").is_none());
        assert_eq!(model_list_req(3, Some("CUR"))["params"]["cursor"], "CUR");
    }

    #[test]
    fn next_cursor_follows_until_null() {
        assert_eq!(next_cursor(&json!({"nextCursor": "abc"})).as_deref(), Some("abc"));
        assert_eq!(next_cursor(&json!({"nextCursor": ""})), None);
        assert_eq!(next_cursor(&json!({})), None);
    }

    #[test]
    fn config_defaults_and_toggle() {
        assert!(ModelListConfig::enabled(&json!({})));
        assert!(!ModelListConfig::enabled(&json!({"model_catalog": false})));
        let cfg = ModelListConfig::from_value(&json!({"model_catalog_poll_ms": 1000}));
        assert_eq!(cfg.poll_interval, Duration::from_millis(1000));
    }
}
