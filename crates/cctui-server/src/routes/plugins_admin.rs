//! Admin plugin management: `GET/POST /api/v1/admin/plugins`,
//! `PATCH/DELETE /api/v1/admin/plugins/{id}`.

use axum::extract::{FromRequest, Multipart, Path, Request, State};
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::{Extension, Json};
use cctui_proto::api::ApiError;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::auth::{AuthContext, Scope};
use crate::plugin_archive::ArchiveError;
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

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
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

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct PluginInstallRequest {
    /// https URL of a `.tar.gz` plugin archive.
    pub url: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct PluginEnableRequest {
    pub enabled: bool,
}

pub async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<AdminPluginInfo>>, StatusCode> {
    ctx.requires(Scope::Admin)?;
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
    let Json(body) = Json::<PluginInstallRequest>::from_request(req, state)
        .await
        .map_err(|e| err(StatusCode::BAD_REQUEST, e.body_text()))?;
    plugin_store::fetch_archive(body.url.trim()).await.map_err(install_error)
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
    use super::{PluginEnableRequest, install, set_enabled, uninstall};
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
    }
}
