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
#[cfg(feature = "ts")]
use ts_rs::TS;

use crate::auth::AuthContext;
use std::collections::BTreeMap;

use crate::plugins::{
    Plugin, PluginInstanceSetting, PluginPage, PluginSetting, enabled_ids, mime_for, plugin_config,
    resolve_static,
};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    /// Tsumikit icon name, when the manifest declares one.
    pub icon: Option<String>,
    /// `/plugins/<id>/<web>?v=<sha8>`, absent for skills-only plugins.
    pub web: Option<String>,
    /// Present when the plugin contributes a full page at `/apps/<id>`.
    pub page: Option<PluginPage>,
    /// Stylesheets to load with the module, as `/plugins/<id>/<path>?v=<sha8>`.
    pub styles: Vec<String>,
    pub skills: Vec<String>,
    /// From the caller's settings `plugins.enabled[id]`.
    pub enabled: bool,
    /// Per-user settings the plugin declares.
    pub settings: Vec<PluginSetting>,
    /// The caller's current values, by setting key (`plugins.config[id]`).
    pub config: BTreeMap<String, String>,
    /// Instance-level settings the admin owns, for display only.
    #[serde(rename = "instanceSettings")]
    pub instance_settings: Vec<PluginInstanceSetting>,
    /// Non-secret instance values, and only for a caller who enabled the
    /// plugin. Secrets are never included.
    #[serde(rename = "instanceSettingValues")]
    pub instance_setting_values: BTreeMap<String, String>,
    /// The plugin's backend is reachable at `/api/v1/plugins/<id>/backend/`.
    pub backend: bool,
}

pub fn plugin_info(
    plugin: &Plugin,
    enabled: bool,
    settings: Option<&Value>,
    instance_values: BTreeMap<String, String>,
) -> PluginInfo {
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
        page: m.page.clone(),
        styles: m
            .styles
            .iter()
            .map(|style| {
                let v = plugin.web_hash.as_deref().unwrap_or("0");
                format!("/plugins/{}/{style}?v={v}", m.id)
            })
            .collect(),
        skills: m.skills.clone(),
        enabled,
        settings: m.settings.clone(),
        config: plugin_config(m, settings),
        instance_settings: m.instance_settings.clone(),
        instance_setting_values: if enabled {
            instance_values
                .into_iter()
                .filter(|(key, _)| m.instance_settings.iter().any(|d| &d.key == key && !d.secret))
                .collect()
        } else {
            BTreeMap::new()
        },
        backend: m.backend.is_some(),
    }
}

/// `instance_values` holds the non-secret instance values per plugin id; a
/// plugin the caller has not enabled gets none of them.
pub fn list_for(
    plugins: &[Plugin],
    settings: Option<&Value>,
    instance_values: &BTreeMap<String, BTreeMap<String, String>>,
) -> Vec<PluginInfo> {
    let enabled = enabled_ids(settings);
    plugins
        .iter()
        .map(|p| {
            plugin_info(
                p,
                enabled.contains(&p.manifest.id),
                settings,
                instance_values.get(&p.manifest.id).cloned().unwrap_or_default(),
            )
        })
        .collect()
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
    crate::plugin_store::sync_or_warn(&state.pool, &state.plugins).await;
    let plugins = state.plugins.all();
    let mut instance_values = BTreeMap::new();
    for plugin in &plugins {
        if plugin.manifest.instance_settings.is_empty() {
            continue;
        }
        let values = crate::plugin_settings::public_values(&state.pool, &plugin.manifest)
            .await
            .map_err(|e| {
                tracing::error!("db error: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        instance_values.insert(plugin.manifest.id.clone(), values);
    }
    Ok(Json(list_for(&plugins, settings.as_ref(), &instance_values)))
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
    if state.plugins.get(&id).is_none() {
        crate::plugin_store::sync_or_warn(&state.pool, &state.plugins).await;
    }
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
        write_plugin(
            root.path(),
            "bb",
            r#","instanceSettings":[{"key":"upstream","label":"U","type":"url"},{"key":"token","label":"T","type":"string","secret":true}],"backend":{"upstreamSetting":"upstream"}"#,
        );
        let plugins = vec![
            load_plugin(&root.path().join("aa")).unwrap(),
            load_plugin(&root.path().join("bb")).unwrap(),
        ];
        let settings = json!({ "plugins": {
            "enabled": { "aa": true, "bb": false },
            "config": { "aa": { "host": "h1", "junk": "x" } }
        } });
        let instance_values = std::collections::BTreeMap::from([(
            "bb".to_owned(),
            std::collections::BTreeMap::from([
                ("upstream".to_owned(), "https://up.example".to_owned()),
                ("token".to_owned(), "leak".to_owned()),
            ]),
        )]);
        let infos = list_for(&plugins, Some(&settings), &instance_values);
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
        assert!(list_for(&plugins, None, &instance_values).iter().all(|i| !i.enabled));

        assert!(!infos[0].backend);
        assert!(infos[1].backend);
        assert_eq!(infos[1].instance_settings.len(), 2);
        assert!(
            infos[1].instance_setting_values.is_empty(),
            "bb is not enabled for this caller, so it gets no instance values"
        );

        let enabled_bb = json!({ "plugins": { "enabled": { "bb": true } } });
        let infos = list_for(&plugins, Some(&enabled_bb), &instance_values);
        assert_eq!(
            infos[1].instance_setting_values.get("upstream").map(String::as_str),
            Some("https://up.example")
        );
        assert!(
            !infos[1].instance_setting_values.contains_key("token"),
            "a secret instance value never reaches a user"
        );
        let json = serde_json::to_value(&infos[1]).unwrap();
        assert!(json["page"].is_null());
        assert_eq!(json["styles"].as_array().map(Vec::len), Some(0));
        assert_eq!(json["instanceSettings"][0]["key"], "upstream");
        assert_eq!(json["instanceSettingValues"]["upstream"], "https://up.example");
    }

    #[test]
    fn a_page_and_its_styles_are_passed_through_cache_busted() {
        let root = tempfile::tempdir().unwrap();
        write_plugin(
            root.path(),
            "cc",
            r#","page":{"title":"Review","icon":"eye"},"styles":["web/style.css"]"#,
        );
        let plugins = vec![load_plugin(&root.path().join("cc")).unwrap()];
        let infos = list_for(&plugins, None, &std::collections::BTreeMap::new());
        let info = &infos[0];
        let page = info.page.as_ref().expect("page passed through");
        assert_eq!(page.title, "Review");
        assert_eq!(page.icon.as_deref(), Some("eye"));
        assert_eq!(info.styles.len(), 1);
        let style = &info.styles[0];
        assert!(style.starts_with("/plugins/cc/web/style.css?v="), "{style}");
        let web = info.web.as_deref().unwrap();
        assert_eq!(
            style.rsplit("?v=").next(),
            web.rsplit("?v=").next(),
            "styles reuse the web bundle's cache-buster"
        );
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
