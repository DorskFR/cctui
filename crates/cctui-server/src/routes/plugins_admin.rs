//! Admin plugin management: `GET/POST /api/v1/admin/plugins`,
//! `PATCH/DELETE /api/v1/admin/plugins/{id}`.

use axum::extract::{FromRequest, Multipart, Path, Request, State};
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::{Extension, Json};
use cctui_proto::api::ApiError;
use serde::Deserialize;

use crate::auth::{AuthContext, Scope};
use crate::plugin_archive::ArchiveError;
use crate::plugin_catalog::{self, CatalogEntry};
use crate::plugin_settings;
use crate::plugin_store::{self, InstallError};
use crate::plugins::{Plugin, PluginSource};
use crate::state::AppState;

pub use cctui_proto::api::plugins::{
    AdminPluginInfo, CatalogPluginInfo, PluginCatalogInstallRequest, PluginEnableRequest,
    PluginInstallRequest, PluginInstanceSettings, PluginInstanceSettingsRequest, PluginProxySecret,
};

type ApiErr = (StatusCode, Json<ApiError>);

fn err(status: StatusCode, msg: impl Into<String>) -> ApiErr {
    (status, Json(ApiError { error: msg.into() }))
}

fn db_err(e: &sqlx::Error) -> ApiErr {
    tracing::error!("db error: {e}");
    err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
}

impl From<&Plugin> for AdminPluginInfo {
    fn from(p: &Plugin) -> Self {
        Self {
            id: p.manifest.id.clone(),
            name: p.manifest.name.clone(),
            description: p.manifest.description.clone(),
            version: p.manifest.version.clone(),
            source: p.source,
            enabled: p.instance_enabled,
            instance_settings: p.manifest.instance_settings.clone(),
            backend: p.manifest.backend.is_some(),
            proxy_secret: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum InstallBody {
    Catalog(PluginCatalogInstallRequest),
    Url(PluginInstallRequest),
}

fn annotate(entry: &CatalogEntry, installed: Option<&str>) -> CatalogPluginInfo {
    CatalogPluginInfo {
        id: entry.id.clone(),
        name: entry.name.clone(),
        description: entry.description.clone(),
        version: entry.version.clone(),
        homepage: entry.homepage.clone(),
        installed_version: installed.map(str::to_owned),
        update_available: installed.is_some_and(|v| v != entry.version),
    }
}

pub async fn catalog(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<CatalogPluginInfo>>, ApiErr> {
    ctx.requires(Scope::Admin).map_err(|s| err(s, "admin only"))?;
    plugin_store::sync_or_warn(&state.pool, &state.plugins).await;
    let installed = state.plugins.all_admin();
    let version_of = |id: &str| {
        installed
            .iter()
            .find(|p| p.manifest.id == id && p.source == PluginSource::Installed)
            .map(|p| p.manifest.version.clone())
    };
    Ok(Json(
        plugin_catalog::entries()
            .await
            .iter()
            .map(|e| annotate(e, version_of(&e.id).as_deref()))
            .collect(),
    ))
}

fn find_entry(list: Vec<CatalogEntry>, id: &str) -> Result<CatalogEntry, ApiErr> {
    list.into_iter()
        .find(|e| e.id == id)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "no catalog plugin with that id"))
}

/// Refuse an archive whose bytes do not hash to the digest the catalog pins.
fn verify_digest(entry: &CatalogEntry, bytes: &[u8]) -> Result<(), ApiErr> {
    let got = plugin_catalog::sha256_hex(bytes);
    if got == entry.sha256 {
        return Ok(());
    }
    tracing::warn!(id = %entry.id, expected = %entry.sha256, got = %got, "catalog archive digest mismatch");
    Err(err(
        StatusCode::BAD_REQUEST,
        format!("{} archive does not match the catalog sha256 (got {got})", entry.id),
    ))
}

async fn catalog_archive(id: &str) -> Result<Vec<u8>, ApiErr> {
    let entry = find_entry(plugin_catalog::entries().await, id)?;
    let bytes = plugin_store::fetch_archive(&entry.url).await.map_err(install_error)?;
    verify_digest(&entry, &bytes)?;
    Ok(bytes)
}

pub async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<AdminPluginInfo>>, StatusCode> {
    ctx.requires(Scope::Admin)?;
    plugin_store::sync_or_warn(&state.pool, &state.plugins).await;
    Ok(Json(state.plugins.all_admin().iter().map(AdminPluginInfo::from).collect()))
}

fn install_error(e: InstallError) -> ApiErr {
    match e {
        InstallError::Archive(ArchiveError::TooLarge | ArchiveError::ExtractedTooLarge) => {
            err(StatusCode::PAYLOAD_TOO_LARGE, e.to_string())
        }
        InstallError::Archive(_) | InstallError::Url(_) | InstallError::Fetch(_) => {
            err(StatusCode::BAD_REQUEST, e.to_string())
        }
        InstallError::Db(e) => db_err(&e),
    }
}

/// The archive bytes of an install request: the `file` part of a multipart
/// body, or the download of a JSON `{url}`.
async fn archive_bytes(state: &AppState, req: Request) -> Result<Vec<u8>, ApiErr> {
    let multipart = req
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("multipart/form-data"));
    if multipart {
        let mut parts = Multipart::from_request(req, state)
            .await
            .map_err(|e| err(StatusCode::BAD_REQUEST, e.to_string()))?;
        while let Some(field) =
            parts.next_field().await.map_err(|e| err(StatusCode::BAD_REQUEST, e.to_string()))?
        {
            if field.name() == Some("file") {
                let bytes = field.bytes().await.map_err(|e| {
                    err(StatusCode::PAYLOAD_TOO_LARGE, format!("upload failed: {e}"))
                })?;
                return Ok(bytes.to_vec());
            }
        }
        return Err(err(StatusCode::BAD_REQUEST, "multipart body has no `file` part"));
    }
    let Json(body) = Json::<InstallBody>::from_request(req, state)
        .await
        .map_err(|e| err(StatusCode::BAD_REQUEST, e.body_text()))?;
    match body {
        InstallBody::Catalog(body) => catalog_archive(body.catalog.trim()).await,
        InstallBody::Url(body) => {
            plugin_store::fetch_archive(body.url.trim()).await.map_err(install_error)
        }
    }
}

pub async fn install(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    req: Request,
) -> Result<Json<AdminPluginInfo>, ApiErr> {
    ctx.requires(Scope::Admin).map_err(|s| err(s, "admin only"))?;
    let bytes = archive_bytes(&state, req).await?;
    let plugin = plugin_store::install(&state.pool, &state.plugins, &bytes, Some(ctx.user_id))
        .await
        .map_err(install_error)?;
    tracing::info!(id = %plugin.manifest.id, version = %plugin.manifest.version, "plugin installed");
    let mut info = AdminPluginInfo::from(&plugin);
    if plugin.manifest.backend.is_some() {
        let (secret, fresh) =
            plugin_settings::ensure_proxy_secret(&state.pool, &plugin.manifest.id)
                .await
                .map_err(|e| db_err(&e))?;
        info.proxy_secret = fresh.then_some(secret);
    }
    Ok(Json(info))
}

pub async fn set_enabled(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(body): Json<PluginEnableRequest>,
) -> Result<Json<AdminPluginInfo>, ApiErr> {
    ctx.requires(Scope::Admin).map_err(|s| err(s, "admin only"))?;
    let known = plugin_store::set_enabled(&state.pool, &state.plugins, &id, body.enabled)
        .await
        .map_err(|e| db_err(&e))?;
    if known && !body.enabled {
        crate::plugin_host_token::revoke_for_all(&state.pool, &state.auth_config, &id)
            .await
            .map_err(|e| db_err(&e))?;
    }
    let plugin = known.then(|| state.plugins.all_admin().into_iter().find(|p| p.manifest.id == id));
    plugin.flatten().map_or_else(
        || Err(err(StatusCode::NOT_FOUND, "no installed plugin with that id")),
        |plugin| Ok(Json(AdminPluginInfo::from(&plugin))),
    )
}

pub async fn uninstall(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiErr> {
    ctx.requires(Scope::Admin).map_err(|s| err(s, "admin only"))?;
    if plugin_store::uninstall(&state.pool, &state.plugins, &id).await.map_err(|e| db_err(&e))? {
        crate::plugin_host_token::revoke_for_all(&state.pool, &state.auth_config, &id)
            .await
            .map_err(|e| db_err(&e))?;
        tracing::info!(id, "plugin uninstalled");
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(err(StatusCode::NOT_FOUND, "no installed plugin with that id"))
    }
}

async fn plugin_by_id(state: &AppState, id: &str) -> Result<Plugin, ApiErr> {
    plugin_store::sync_or_warn(&state.pool, &state.plugins).await;
    state
        .plugins
        .all_admin()
        .into_iter()
        .find(|p| p.manifest.id == id)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "no plugin with that id"))
}

async fn settings_view(
    state: &AppState,
    plugin: &Plugin,
) -> Result<PluginInstanceSettings, ApiErr> {
    let m = &plugin.manifest;
    let view = plugin_settings::admin_view(&state.pool, m).await.map_err(|e| db_err(&e))?;
    let proxy_secret_set =
        plugin_settings::proxy_secret(&state.pool, &m.id).await.map_err(|e| db_err(&e))?.is_some();
    Ok(PluginInstanceSettings {
        id: m.id.clone(),
        instance_settings: m.instance_settings.clone(),
        values: view.values,
        secrets_set: view.secrets_set,
        backend_upstream_setting: m.backend.as_ref().map(|b| b.upstream_setting.clone()),
        proxy_secret_set,
    })
}

pub async fn get_settings(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<PluginInstanceSettings>, ApiErr> {
    ctx.requires(Scope::Admin).map_err(|s| err(s, "admin only"))?;
    let plugin = plugin_by_id(&state, &id).await?;
    Ok(Json(settings_view(&state, &plugin).await?))
}

pub async fn put_settings(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(body): Json<PluginInstanceSettingsRequest>,
) -> Result<Json<PluginInstanceSettings>, ApiErr> {
    ctx.requires(Scope::Admin).map_err(|s| err(s, "admin only"))?;
    let plugin = plugin_by_id(&state, &id).await?;
    plugin_settings::write(&state.pool, &plugin.manifest, &body.values).await.map_err(
        |e| match e {
            plugin_settings::SettingsError::Db(e) => db_err(&e),
            other => err(StatusCode::BAD_REQUEST, other.to_string()),
        },
    )?;
    tracing::info!(id = %plugin.manifest.id, "plugin instance settings updated");
    Ok(Json(settings_view(&state, &plugin).await?))
}

pub async fn rotate_proxy_secret(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<PluginProxySecret>, ApiErr> {
    ctx.requires(Scope::Admin).map_err(|s| err(s, "admin only"))?;
    let plugin = plugin_by_id(&state, &id).await?;
    let secret = plugin_settings::rotate_proxy_secret(&state.pool, &plugin.manifest.id)
        .await
        .map_err(|e| db_err(&e))?;
    tracing::info!(id = %plugin.manifest.id, "plugin proxy secret rotated");
    Ok(Json(PluginProxySecret { id: plugin.manifest.id.clone(), secret }))
}

#[cfg(test)]
mod tests {
    use super::{
        CatalogEntry, InstallBody, PluginEnableRequest, PluginInstanceSettingsRequest, annotate,
        catalog, find_entry, get_settings, install, put_settings, rotate_proxy_secret, set_enabled,
        uninstall, verify_digest,
    };
    use crate::auth::AuthContext;
    use crate::state::AppState;
    use axum::extract::{Path, Request, State};
    use axum::http::StatusCode;
    use axum::{Extension, Json};
    use uuid::Uuid;

    fn user() -> AuthContext {
        AuthContext {
            user_id: Uuid::new_v4(),
            key_id: Uuid::nil(),
            machine_id: None,
            scopes: std::iter::once(crate::auth::Scope::Read).collect(),
        }
    }

    fn state() -> AppState {
        AppState::for_test(sqlx::PgPool::connect_lazy("postgres://unused@localhost/none").unwrap())
    }

    #[tokio::test]
    async fn non_admins_get_403_before_any_work() {
        let req = Request::builder().body(axum::body::Body::empty()).unwrap();
        let (status, _) = install(State(state()), Extension(user()), req).await.unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = set_enabled(
            State(state()),
            Extension(user()),
            Path("demo".into()),
            Json(PluginEnableRequest { enabled: true }),
        )
        .await
        .unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) =
            uninstall(State(state()), Extension(user()), Path("demo".into())).await.unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = catalog(State(state()), Extension(user())).await.unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) =
            get_settings(State(state()), Extension(user()), Path("demo".into())).await.unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = put_settings(
            State(state()),
            Extension(user()),
            Path("demo".into()),
            Json(PluginInstanceSettingsRequest { values: std::collections::BTreeMap::new() }),
        )
        .await
        .unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) =
            rotate_proxy_secret(State(state()), Extension(user()), Path("demo".into()))
                .await
                .unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[test]
    fn an_admin_listing_never_carries_a_proxy_secret() {
        let plugin = crate::plugin_archive::load_archive(
            &crate::plugin_archive::test_support::demo_tgz(None, "1.0.0"),
            true,
        )
        .unwrap();
        let info = super::AdminPluginInfo::from(&plugin);
        assert!(info.proxy_secret.is_none());
        let json = serde_json::to_value(&info).unwrap();
        assert!(json.get("proxy_secret").is_none());
    }

    fn entry(version: &str) -> CatalogEntry {
        CatalogEntry {
            id: "demo".into(),
            name: "Demo".into(),
            description: "d".into(),
            version: version.into(),
            url: "https://example.com/demo.tgz".into(),
            sha256: crate::plugin_catalog::sha256_hex(b"archive"),
            homepage: Some("https://example.com".into()),
        }
    }

    #[test]
    fn annotations_say_install_update_or_up_to_date() {
        let fresh = annotate(&entry("1.1.0"), None);
        assert_eq!(fresh.installed_version, None);
        assert!(!fresh.update_available);
        assert_eq!(fresh.homepage.as_deref(), Some("https://example.com"));

        let stale = annotate(&entry("1.1.0"), Some("1.0.0"));
        assert_eq!(stale.installed_version.as_deref(), Some("1.0.0"));
        assert!(stale.update_available);

        let current = annotate(&entry("1.1.0"), Some("1.1.0"));
        assert_eq!(current.installed_version.as_deref(), Some("1.1.0"));
        assert!(!current.update_available);
    }

    #[test]
    fn an_unknown_catalog_id_is_a_404_and_a_known_one_resolves_server_side() {
        let list = vec![entry("1.0.0")];
        let (status, _) = find_entry(list.clone(), "nope").unwrap_err();
        assert_eq!(status, StatusCode::NOT_FOUND);
        let found = find_entry(list, "demo").unwrap();
        assert_eq!(found.url, "https://example.com/demo.tgz");
        assert_eq!(found.sha256, crate::plugin_catalog::sha256_hex(b"archive"));
    }

    #[test]
    fn the_pinned_digest_gates_the_archive() {
        let entry = entry("1.0.0");
        verify_digest(&entry, b"archive").unwrap();
        let (status, body) = verify_digest(&entry, b"tampered").unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.0.error.contains("does not match the catalog sha256"), "{}", body.0.error);
    }

    #[test]
    fn the_committed_catalog_pins_yubisashi_for_a_one_click_install() {
        let entry = find_entry(crate::plugin_catalog::embedded(), "yubisashi").unwrap();
        assert!(entry.url.starts_with("https://"));
        assert_eq!(entry.sha256.len(), 64);
    }

    #[test]
    fn an_install_body_is_a_catalog_id_or_a_url_never_both_read_from_the_client() {
        let catalog: InstallBody = serde_json::from_str(r#"{"catalog":"yubisashi"}"#).unwrap();
        assert!(matches!(catalog, InstallBody::Catalog(b) if b.catalog == "yubisashi"));
        let url: InstallBody =
            serde_json::from_str(r#"{"url":"https://e.example/p.tgz"}"#).unwrap();
        assert!(matches!(url, InstallBody::Url(b) if b.url == "https://e.example/p.tgz"));
        let spoofed: InstallBody =
            serde_json::from_str(r#"{"catalog":"yubisashi","url":"https://evil.example/p.tgz"}"#)
                .unwrap();
        assert!(
            matches!(spoofed, InstallBody::Catalog(b) if b.catalog == "yubisashi"),
            "a catalog install ignores any client-supplied url"
        );
    }
}
