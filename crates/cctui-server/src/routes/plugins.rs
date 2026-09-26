//! `GET /api/v1/plugins`, admin `POST /api/v1/plugins/rescan`, and the public
//! `GET /plugins/{id}/{*path}` static route the webui imports plugin modules from.

use axum::extract::{Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use ts_rs::TS;

use crate::auth::AuthContext;
use std::collections::BTreeMap;

use crate::plugins::{Plugin, PluginSetting, enabled_ids, mime_for, plugin_config, resolve_static};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    /// Tsumikit icon name, when the manifest declares one.
    pub icon: Option<String>,
    /// `/plugins/<id>/<web>?v=<sha8>`, absent for skills-only plugins.
    pub web: Option<String>,
    pub skills: Vec<String>,
    /// From the caller's settings `plugins.enabled[id]`.
    pub enabled: bool,
    /// Per-user settings the plugin declares.
    pub settings: Vec<PluginSetting>,
    /// The caller's current values, by setting key (`plugins.config[id]`).
    pub config: BTreeMap<String, String>,
}

pub fn plugin_info(plugin: &Plugin, enabled: bool, settings: Option<&Value>) -> PluginInfo {
    let m = &plugin.manifest;
    PluginInfo {
        id: m.id.clone(),
        name: m.name.clone(),
        description: m.description.clone(),
        version: m.version.clone(),
        icon: m.icon.clone(),
        web: m.web.as_ref().map(|web| {
            let v = plugin.web_hash.as_deref().unwrap_or("0");
            format!("/plugins/{}/{web}?v={v}", m.id)
        }),
        skills: m.skills.clone(),
        enabled,
        settings: m.settings.clone(),
        config: plugin_config(m, settings),
    }
}

pub fn list_for(plugins: &[Plugin], settings: Option<&Value>) -> Vec<PluginInfo> {
    let enabled = enabled_ids(settings);
    plugins.iter().map(|p| plugin_info(p, enabled.contains(&p.manifest.id), settings)).collect()
}

pub async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<PluginInfo>>, StatusCode> {
    ctx.requires(crate::auth::Scope::Read)?;
    let settings: Option<Value> =
        sqlx::query_scalar("SELECT data FROM user_settings WHERE user_id = $1")
            .bind(ctx.user_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| {
                tracing::error!("db error: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    Ok(Json(list_for(&state.plugins.all(), settings.as_ref())))
}

#[derive(Debug, Serialize)]
pub struct RescanResponse {
    pub enabled: bool,
    pub installed: usize,
}

pub async fn rescan(State(state): State<AppState>) -> Json<RescanResponse> {
    let installed = state.plugins.rescan();
    Json(RescanResponse { enabled: state.plugins.enabled(), installed })
}

/// Response for `GET /plugins/{id}/{path}` from bytes already read from disk.
pub fn static_response(req_headers: &HeaderMap, path: &str, body: Vec<u8>) -> Response {
    let etag = format!("\"{}\"", &hex::encode(Sha256::digest(&body))[..32]);
    let matched = req_headers
        .get(IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|inm| inm.split(',').any(|t| t.trim().trim_start_matches("W/") == etag));
    let mut headers = HeaderMap::new();
    headers.insert(ETAG, etag.parse().expect("hex etag is a valid header value"));
    headers.insert(CACHE_CONTROL, "no-cache".parse().expect("static header"));
    headers.insert("x-content-type-options", "nosniff".parse().expect("static header"));
    if matched {
        return (StatusCode::NOT_MODIFIED, headers).into_response();
    }
    headers.insert(CONTENT_TYPE, mime_for(path).parse().expect("static mime"));
    (StatusCode::OK, headers, body).into_response()
}

pub async fn static_file(
    State(state): State<AppState>,
    Path((id, path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let Some(plugin) = state.plugins.get(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let read =
        tokio::task::spawn_blocking(move || resolve_static(&plugin, &path).map(|b| (path, b)));
    match read.await {
        Ok(Some((path, body))) => static_response(&headers, &path, body),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::{list_for, static_response};
    use crate::plugins::load_plugin;
    use crate::plugins::test_support::write_plugin;
    use axum::http::{HeaderMap, StatusCode};
    use serde_json::json;

    #[test]
    fn list_marks_enabled_from_settings_and_cache_busts_web() {
        let root = tempfile::tempdir().unwrap();
        write_plugin(
            root.path(),
            "aa",
            r#","icon":"eye","settings":[{"key":"host","label":"Host","env":"AA_HOST","type":"string"}]"#,
        );
        write_plugin(root.path(), "bb", "");
        let plugins = vec![
            load_plugin(&root.path().join("aa")).unwrap(),
            load_plugin(&root.path().join("bb")).unwrap(),
        ];
        let settings = json!({ "plugins": {
            "enabled": { "aa": true, "bb": false },
            "config": { "aa": { "host": "h1", "junk": "x" } }
        } });
        let infos = list_for(&plugins, Some(&settings));
        assert_eq!(infos.len(), 2);
        assert!(infos[0].enabled);
        assert_eq!(infos[0].settings.len(), 1);
        assert_eq!(infos[0].settings[0].env, "AA_HOST");
        assert_eq!(infos[0].config.get("host").map(String::as_str), Some("h1"));
        assert!(!infos[0].config.contains_key("junk"));
        assert!(infos[1].settings.is_empty());
        assert!(infos[1].config.is_empty());
        assert_eq!(infos[0].icon.as_deref(), Some("eye"));
        assert_eq!(infos[0].skills, vec!["aa"]);
        let web = infos[0].web.as_deref().unwrap();
        assert!(web.starts_with("/plugins/aa/web/index.js?v="), "{web}");
        assert_eq!(web.len(), "/plugins/aa/web/index.js?v=".len() + 8);
        assert!(!infos[1].enabled);
        assert!(list_for(&plugins, None).iter().all(|i| !i.enabled));
    }

    #[tokio::test]
    async fn static_route_serves_installed_plugins_only_while_enabled() {
        use axum::extract::{Path, State};
        let state = crate::state::AppState::for_test(
            sqlx::PgPool::connect_lazy("postgres://unused@localhost/none").unwrap(),
        );
        let plugin = crate::plugin_archive::load_archive(
            &crate::plugin_archive::test_support::demo_tgz(None, "1.0.0"),
            true,
        )
        .unwrap();
        state.plugins.upsert_installed(plugin);
        let get = |path: &str| {
            super::static_file(
                State(state.clone()),
                Path(("demo".to_owned(), path.to_owned())),
                HeaderMap::new(),
            )
        };
        let resp = get("web/index.js").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()["content-type"], "text/javascript; charset=utf-8");
        assert_eq!(get("skills/demo/SKILL.md").await.status(), StatusCode::OK);
        assert_eq!(get("../plugin.json").await.status(), StatusCode::NOT_FOUND);
        assert_eq!(get("web/nope.js").await.status(), StatusCode::NOT_FOUND);
        state.plugins.set_installed_enabled("demo", false);
        assert_eq!(get("web/index.js").await.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn static_response_sets_mime_nosniff_and_honours_etag() {
        let resp = static_response(&HeaderMap::new(), "web/index.js", b"x".to_vec());
        assert_eq!(resp.status(), StatusCode::OK);
        let h = resp.headers();
        assert_eq!(h["content-type"], "text/javascript; charset=utf-8");
        assert_eq!(h["x-content-type-options"], "nosniff");
        assert_eq!(h["cache-control"], "no-cache");
        let etag = h["etag"].to_str().unwrap().to_owned();

        let mut req = HeaderMap::new();
        req.insert("if-none-match", etag.parse().unwrap());
        let resp = static_response(&req, "web/index.js", b"x".to_vec());
        assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);

        let resp = static_response(&HeaderMap::new(), "web/a.css", b"y".to_vec());
        assert_eq!(resp.headers()["content-type"], "text/css; charset=utf-8");
    }
}
