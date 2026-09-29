//! Admin plugin management: `GET/POST /api/v1/admin/plugins`,
//! `PATCH/DELETE /api/v1/admin/plugins/{id}`.

use axum::extract::{FromRequest, Multipart, Path, Request, State};
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::{Extension, Json};
use cctui_proto::api::ApiError;
use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

use crate::auth::{AuthContext, Scope};
use crate::plugin_archive::ArchiveError;
use crate::plugin_catalog::{self, CatalogEntry};
use crate::plugin_store::{self, InstallError};
use crate::plugins::{Plugin, PluginSource};
use crate::state::AppState;

type ApiErr = (StatusCode, Json<ApiError>);

fn err(status: StatusCode, msg: impl Into<String>) -> ApiErr {
    (status, Json(ApiError { error: msg.into() }))
}

fn db_err(e: &sqlx::Error) -> ApiErr {
    tracing::error!("db error: {e}");
    err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct AdminPluginInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub source: PluginSource,
    /// The instance-wide toggle; directory plugins are always on.
    pub enabled: bool,
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
        }
    }
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct PluginInstallRequest {
    /// https URL of a `.tar.gz` plugin archive.
    pub url: String,
}

/// Install a published plugin by catalog id; the server resolves its url and
/// sha256 itself and never trusts client-supplied ones.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct PluginCatalogInstallRequest {
    pub catalog: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum InstallBody {
    Catalog(PluginCatalogInstallRequest),
    Url(PluginInstallRequest),
}

/// A catalog entry as the admin UI sees it: the published metadata plus what
/// this instance has installed.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct CatalogPluginInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub homepage: Option<String>,
    /// `None` until this instance installs it.
    pub installed_version: Option<String>,
    /// The catalog version differs from the installed one.
    pub update_available: bool,
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

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct PluginEnableRequest {
    pub enabled: bool,
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
    Ok(Json(AdminPluginInfo::from(&plugin)))
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
        tracing::info!(id, "plugin uninstalled");
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(err(StatusCode::NOT_FOUND, "no installed plugin with that id"))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CatalogEntry, InstallBody, PluginEnableRequest, annotate, catalog, find_entry, install,
        set_enabled, uninstall, verify_digest,
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
