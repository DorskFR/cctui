use cctui_proto::api::bookmarks::{Bookmark, CreateBookmark};
use cctui_proto::api::cache_loss::DailyCacheLoss;
use cctui_proto::api::langfuse::LangfuseSessionUsage;
use cctui_proto::api::machine_resources::MachineResourcesRow;
use cctui_proto::api::me::MeResponse;
use cctui_proto::api::profiles::{
    CreateProfileRequest, ReorderProfilesRequest, SessionProfile, UpdateProfileRequest,
};
use cctui_proto::api::routes::{Method, Route, by_id};
use cctui_proto::api::settings::SettingsPayload;
use cctui_proto::api::{
    AttachLabelRequest, AutoApproveRequest, CreateLabelRequest, DispatchResponse, ForkRequest,
    ForkResponse, Label, LabelListResponse, RenameRequest, SessionListItem, SessionListResponse,
    SessionStats, SetModelRequest, SpawnRequest, SpawnResponse, StageFilesResponse,
    TokenUsageWindows, UpdateLabelRequest, UsageAnalytics,
};
use cctui_proto::diagnose::SessionDiagnoseResponse;
use cctui_proto::drafts::{Draft, DraftList, PutDraftRequest};
use cctui_proto::harness_models::HarnessModels;
use cctui_proto::models::MessagePin;
use reqwest::StatusCode;
use reqwest::header::{ETAG, IF_NONE_MATCH};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ClientError;
use crate::usage::{AccountUsageEntry, PoolUsageView};

/// Envoy strips `ETag` off responses it compresses; the server mirrors it here.
const ETAG_MIRROR: &str = "x-etag";

/// Reaching a listening server is fast or not happening at all.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Enough for a slow server paging a long transcript. Without it a hung request
/// never returns and the caller cannot tell that from a slow one.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(1);

/// Uploads push attachment bytes, so they are bounded by the link rather than
/// by how long an answer should take.
const UPLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(5);

/// One conversation row, addressed by `seq`.
///
/// `event` is the untouched server object (it still carries `seq`, `ts` and
/// `turn_id`), so deserializing it as an `AgentEvent` loses nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationRow {
    pub seq: i64,
    pub ts: Option<i64>,
    pub turn_id: Option<uuid::Uuid>,
    pub event: Value,
}

impl<'de> serde::Deserialize<'de> for ConversationRow {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let event = Value::deserialize(de)?;
        let seq = event.get("seq").and_then(Value::as_i64).unwrap_or_default();
        let ts = event.get("ts").and_then(Value::as_i64);
        let turn_id = event
            .get("turn_id")
            .and_then(Value::as_str)
            .and_then(|s| uuid::Uuid::parse_str(s).ok());
        Ok(Self { seq, ts, turn_id, event })
    }
}

/// One permission request the server is still holding.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct PendingPermissionItem {
    pub session_id: String,
    pub request_id: String,
    pub tool_name: String,
    pub description: String,
    pub input_preview: String,
}

/// Which slice of a transcript to fetch. Rows always come back oldest-first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Page {
    /// Exclusive upper `seq` bound.
    pub before: Option<i64>,
    /// Exclusive lower `seq` bound.
    pub after: Option<i64>,
    pub limit: Option<i64>,
}

impl Page {
    /// The newest `limit` rows.
    #[must_use]
    pub const fn latest(limit: i64) -> Self {
        Self { before: None, after: None, limit: Some(limit) }
    }

    /// The `limit` rows immediately before `seq`.
    #[must_use]
    pub const fn before(seq: i64, limit: i64) -> Self {
        Self { before: Some(seq), after: None, limit: Some(limit) }
    }

    /// Everything after `seq` — the reconnect gap.
    #[must_use]
    pub const fn after(seq: i64) -> Self {
        Self { before: None, after: Some(seq), limit: None }
    }

    fn query(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        if let Some(before) = self.before {
            out.push(("before", before.to_string()));
        }
        if let Some(after) = self.after {
            out.push(("after", after.to_string()));
        }
        if let Some(limit) = self.limit {
            out.push(("limit", limit.to_string()));
        }
        out
    }
}

/// The outcome of a conditional conversation fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationFetch {
    NotModified,
    Page { rows: Vec<ConversationRow>, etag: Option<String>, has_more: bool },
}

/// One file handed to `stage_session_files`.
pub struct UploadFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// A refused file read, kept as data so the caller can word it. `status` is 0
/// for a transport failure, which the viewer words as "network".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRefusal {
    pub status: u16,
    pub detail: String,
    /// The route's structured list; empty from a server that sends none, and
    /// then the prose in `detail` is all there is.
    pub allowed_folders: Vec<String>,
}

impl FileRefusal {
    #[must_use]
    pub const fn network() -> Self {
        Self { status: 0, detail: String::new(), allowed_folders: Vec::new() }
    }

    /// Pull `error` and `allowed_folders` out of a refusal body; a body that is
    /// not the expected JSON leaves both empty rather than failing the read.
    #[must_use]
    pub fn parse(status: u16, body: &str) -> Self {
        #[derive(Deserialize, Default)]
        struct Body {
            #[serde(default)]
            error: String,
            #[serde(default)]
            allowed_folders: Vec<String>,
        }
        let parsed: Body = serde_json::from_str(body).unwrap_or_default();
        Self { status, detail: parsed.error, allowed_folders: parsed.allowed_folders }
    }
}

/// What a machine-file read produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileRead {
    Ok { content_type: String, bytes: Vec<u8> },
    Refused(FileRefusal),
}

/// Where a linked path actually lives, so a viewer refused on its own machine
/// can re-ask the machine that owns it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LinkedFileOwner {
    pub session_id: String,
    pub machine_id: String,
}

/// One enrolled dispatcher, as `GET /dispatchers` reports it.
///
/// Mirrors `cctui-server`'s `DispatcherInfo`, which lives in the server crate:
/// the TUI cannot depend on it, and a reader only needs these fields.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Dispatcher {
    pub id: String,
    pub name: String,
    /// `kubernetes` | `docker` | `http`, as the binary reported at enroll.
    pub kind: String,
    pub liveness: cctui_proto::models::MachineLiveness,
    /// A live socket is registered right now, which `liveness` alone cannot say.
    pub connected: bool,
    pub last_seen_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    pub default_account: Option<String>,
    #[serde(default)]
    pub default_pool: Option<String>,
}

/// What `POST /dispatcher/enroll` takes.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EnrollDispatcher {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool: Option<String>,
}

/// The one-shot reply to an enrollment.
#[derive(Debug, Clone, Deserialize)]
pub struct EnrolledDispatcher {
    pub dispatcher_id: String,
    /// Shown once and never persisted: the server keeps only a hash.
    pub dispatcher_key: String,
}

/// A rename or a rebind; an omitted field is left alone.
#[derive(Debug, Clone, Default, Serialize)]
pub struct UpdateDispatcher {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool: Option<String>,
}

/// Typed REST client. Every URL is built from
/// `cctui_proto::api::routes::ROUTES`, so a renamed path cannot strand it.
pub struct Client {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

impl Client {
    #[must_use]
    pub fn new(base_url: impl AsRef<str>, token: impl Into<String>) -> Self {
        Self::with_timeouts(base_url, token, CONNECT_TIMEOUT, REQUEST_TIMEOUT)
    }

    /// `new` with the bounds spelled out, so a caller that knows its link is
    /// slow — or a test that needs a short deadline — can say so.
    #[must_use]
    pub fn with_timeouts(
        base_url: impl AsRef<str>,
        token: impl Into<String>,
        connect: std::time::Duration,
        request: std::time::Duration,
    ) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(connect)
            .timeout(request)
            .build()
            .expect("a reqwest client with timeouts, as reqwest::Client::new also expects");
        Self {
            base_url: base_url.as_ref().trim_end_matches('/').to_owned(),
            token: token.into(),
            http,
        }
    }

    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The absolute URL of `route` with its path params substituted.
    #[must_use]
    pub fn url_for(&self, route: &Route, params: &[(&str, &str)]) -> String {
        format!("{}{}", self.base_url, route.url(params))
    }

    /// The `ws://`/`wss://` URL of the client event stream.
    #[must_use]
    pub fn ws_url(&self) -> String {
        let origin = match self.base_url.split_once("://") {
            Some(("https", rest)) => format!("wss://{rest}"),
            Some(("http", rest)) => format!("ws://{rest}"),
            _ => self.base_url.clone(),
        };
        format!("{origin}{}/ws", cctui_proto::api::routes::API_PREFIX)
    }

    pub fn route(id: &str) -> Result<&'static Route, ClientError> {
        by_id(id).ok_or_else(|| ClientError::UnknownRoute(id.to_owned()))
    }

    async fn send(
        &self,
        route: &'static Route,
        params: &[(&str, &str)],
        query: &[(&'static str, String)],
        body: Option<&Value>,
        if_none_match: Option<&str>,
    ) -> Result<reqwest::Response, ClientError> {
        let method = match route.method {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
            Method::Put => reqwest::Method::PUT,
            Method::Patch => reqwest::Method::PATCH,
            Method::Delete => reqwest::Method::DELETE,
        };
        let mut request =
            self.http.request(method, self.url_for(route, params)).bearer_auth(&self.token);
        if !query.is_empty() {
            request = request.query(query);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        if let Some(etag) = if_none_match {
            request = request.header(IF_NONE_MATCH, etag);
        }
        let resp = request
            .send()
            .await
            .map_err(|source| ClientError::Transport { route: route.id, source })?;
        check_status(route.id, resp).await
    }

    async fn json<R: DeserializeOwned>(
        &self,
        route: &'static Route,
        params: &[(&str, &str)],
        query: &[(&'static str, String)],
        body: Option<&Value>,
    ) -> Result<R, ClientError> {
        let resp = self.send(route, params, query, body, None).await?;
        decode_body(route.id, resp).await
    }

    async fn unit(
        &self,
        route: &'static Route,
        params: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Result<(), ClientError> {
        let _ = self.send(route, params, &[], body, None).await?;
        Ok(())
    }

    // --- generic, by route id ---

    /// `GET` any route by id. An empty body decodes to `Value::Null`.
    pub async fn get(
        &self,
        route_id: &str,
        params: &[(&str, &str)],
        query: &[(&'static str, String)],
    ) -> Result<Value, ClientError> {
        self.json(Self::route(route_id)?, params, query, None).await
    }

    pub async fn post(
        &self,
        route_id: &str,
        params: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Result<Value, ClientError> {
        self.json(Self::route(route_id)?, params, &[], body).await
    }

    pub async fn patch(
        &self,
        route_id: &str,
        params: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Result<Value, ClientError> {
        self.json(Self::route(route_id)?, params, &[], body).await
    }

    pub async fn delete(
        &self,
        route_id: &str,
        params: &[(&str, &str)],
    ) -> Result<Value, ClientError> {
        self.json(Self::route(route_id)?, params, &[], None).await
    }

    /// `GET` any route by id, deserialized into `R`.
    pub async fn get_as<R: DeserializeOwned>(
        &self,
        route_id: &str,
        params: &[(&str, &str)],
    ) -> Result<R, ClientError> {
        self.json(Self::route(route_id)?, params, &[], None).await
    }

    /// `POST` any route by id, deserialized into `R`.
    pub async fn post_as<B: Serialize + Sync, R: DeserializeOwned>(
        &self,
        route_id: &str,
        params: &[(&str, &str)],
        body: &B,
    ) -> Result<R, ClientError> {
        let route = Self::route(route_id)?;
        let body = serde_json::to_value(body)
            .map_err(|source| ClientError::Decode { route: route.id, source })?;
        self.json(route, params, &[], Some(&body)).await
    }

    // --- named routes ---

    pub async fn list_sessions(&self) -> Result<SessionListResponse, ClientError> {
        self.json(Self::route("get_sessions")?, &[], &[], None).await
    }

    pub async fn get_session(&self, session_id: &str) -> Result<SessionListItem, ClientError> {
        self.json(Self::route("get_sessions_by_id")?, &[("id", session_id)], &[], None).await
    }

    /// `timezone` is an IANA name: the today/week/month counts are local
    /// calendar periods and the server defaults to UTC without it.
    pub async fn session_stats(&self, timezone: &str) -> Result<SessionStats, ClientError> {
        self.json(
            Self::route("get_sessions_stats")?,
            &[],
            &[("timezone", timezone.to_owned())],
            None,
        )
        .await
    }

    pub async fn me(&self) -> Result<MeResponse, ClientError> {
        self.json(Self::route("get_me")?, &[], &[], None).await
    }

    pub async fn conversation(
        &self,
        session_id: &str,
        page: Page,
        etag: Option<&str>,
    ) -> Result<ConversationFetch, ClientError> {
        let route = Self::route("get_sessions_by_id_conversation")?;
        let resp = self.send(route, &[("id", session_id)], &page.query(), None, etag).await?;
        if resp.status() == StatusCode::NOT_MODIFIED {
            return Ok(ConversationFetch::NotModified);
        }
        let etag = read_etag(&resp);
        let rows: Vec<ConversationRow> = decode_body(route.id, resp).await?;
        let has_more =
            page.limit.is_some_and(|limit| i64::try_from(rows.len()).unwrap_or(i64::MAX) >= limit);
        Ok(ConversationFetch::Page { rows, etag, has_more })
    }

    /// Change a running session's model and/or effort. An empty string means
    /// the harness default; `None` leaves that dial alone.
    pub async fn set_model(
        &self,
        session_id: &str,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> Result<(), ClientError> {
        let route = Self::route("post_sessions_by_id_set_model")?;
        let body = serde_json::to_value(SetModelRequest {
            model: model.map(str::to_owned),
            effort: effort.map(str::to_owned),
        })
        .map_err(|source| ClientError::Decode { route: route.id, source })?;
        self.unit(route, &[("id", session_id)], Some(&body)).await
    }

    /// Fork a session. `ForkRequest::default()` inherits every dial, so a
    /// whole-session fork needs no options; the dialog fills in what it was
    /// given.
    pub async fn fork(
        &self,
        session_id: &str,
        request: &ForkRequest,
    ) -> Result<ForkResponse, ClientError> {
        let route = Self::route("post_sessions_by_id_fork")?;
        let body = serde_json::to_value(request)
            .map_err(|source| ClientError::Decode { route: route.id, source })?;
        self.json(route, &[("id", session_id)], &[], Some(&body)).await
    }

    /// The dispatch targets a spawn can pick, `GET /sessions/dispatchers`.
    pub async fn spawn_dispatchers(&self) -> Result<Vec<String>, ClientError> {
        self.json(Self::route("get_sessions_dispatchers")?, &[], &[], None).await
    }

    /// Hand a job to a dispatcher. The body is
    /// `cctui_clientcore::dispatch::build_dispatch_body`, shared with the web
    /// UI, so it is passed through as built.
    pub async fn dispatch(&self, body: &Value) -> Result<DispatchResponse, ClientError> {
        self.json(Self::route("post_sessions_dispatch")?, &[], &[], Some(body)).await
    }

    /// Resume an exited session. The server re-mints the gateway env, so the
    /// request carries nothing.
    pub async fn resume(&self, session_id: &str) -> Result<(), ClientError> {
        self.unit(Self::route("post_sessions_by_id_resume")?, &[("id", session_id)], None).await
    }

    /// The model and effort lists a picker for `harness` should offer.
    ///
    /// `machine_id` narrows the codex catalog and is only sent when it is a
    /// uuid, which is what the route accepts; `model` scopes the effort list.
    pub async fn harness_models(
        &self,
        harness: &str,
        machine_id: Option<&str>,
        model: &str,
    ) -> Result<HarnessModels, ClientError> {
        let mut query: Vec<(&'static str, String)> = Vec::new();
        if let Some(machine) = machine_id.filter(|m| uuid::Uuid::parse_str(m).is_ok()) {
            query.push(("machine_id", machine.to_owned()));
        }
        if !model.is_empty() {
            query.push(("model", model.to_owned()));
        }
        self.json(Self::route("get_models_by_harness")?, &[("harness", harness)], &query, None)
            .await
    }

    pub async fn interrupt(&self, session_id: &str) -> Result<(), ClientError> {
        self.unit(Self::route("post_sessions_by_id_interrupt")?, &[("id", session_id)], None).await
    }

    pub async fn diagnose(&self, session_id: &str) -> Result<SessionDiagnoseResponse, ClientError> {
        self.json(Self::route("get_sessions_by_id_diagnose")?, &[("id", session_id)], &[], None)
            .await
    }

    /// The caller's daemon machines with their last resource snapshot. Readable
    /// without admin: the row set is already owner-filtered server-side, so a
    /// single-user install sees its own machine and nothing 403s.
    pub async fn machines(&self) -> Result<Vec<MachineResourcesRow>, ClientError> {
        self.json(Self::route("get_machines_resources")?, &[], &[], None).await
    }

    /// Token totals across the rolling windows. `tz_offset` is the caller's
    /// `Date.getTimezoneOffset()` equivalent — minutes to subtract from UTC —
    /// which anchors `today` to the local calendar day.
    pub async fn session_token_stats(
        &self,
        tz_offset: i32,
    ) -> Result<TokenUsageWindows, ClientError> {
        self.json(
            Self::route("get_sessions_stats_tokens")?,
            &[],
            &[("tz_offset", tz_offset.to_string())],
            None,
        )
        .await
    }

    /// Tokens over time, per model and per hour-of-week, over the last `days`.
    pub async fn session_usage_analytics(
        &self,
        days: u32,
        tz_offset: i32,
    ) -> Result<UsageAnalytics, ClientError> {
        self.json(
            Self::route("get_sessions_stats_usage")?,
            &[],
            &[("days", days.to_string()), ("tz_offset", tz_offset.to_string())],
            None,
        )
        .await
    }

    /// Dollars and tokens lost to prompt-cache busts, per local day.
    pub async fn cache_loss(
        &self,
        days: u32,
        tz_offset: i32,
    ) -> Result<Vec<DailyCacheLoss>, ClientError> {
        self.json(
            Self::route("get_sessions_stats_cache_busts")?,
            &[],
            &[("days", days.to_string()), ("tz_offset", tz_offset.to_string())],
            None,
        )
        .await
    }

    /// The session's Langfuse cost rollup. Errors when the sink is
    /// unconfigured, so the caller hides the cost line rather than showing a
    /// zero.
    pub async fn session_langfuse(
        &self,
        session_id: &str,
    ) -> Result<LangfuseSessionUsage, ClientError> {
        self.json(Self::route("get_sessions_by_id_langfuse")?, &[("id", session_id)], &[], None)
            .await
    }

    /// Usage windows of every provider credential the caller owns, in one call.
    pub async fn accounts_usage(&self) -> Result<Vec<AccountUsageEntry>, ClientError> {
        self.json(Self::route("get_accounts_usage")?, &[], &[], None).await
    }

    /// Every pool's windows aggregated per provider family.
    pub async fn account_pools_usage(&self) -> Result<Vec<PoolUsageView>, ClientError> {
        self.json(Self::route("get_account_pools_usage")?, &[], &[], None).await
    }

    /// A session's per-family account bindings.
    pub async fn session_bindings(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::session_bindings::SessionBinding>, ClientError> {
        self.json(Self::route("get_sessions_by_id_bindings")?, &[("id", session_id)], &[], None)
            .await
    }

    /// Rebind the session's credential in `family` to `account` (a name, an
    /// identity id, or a credential id, which names its own family).
    pub async fn switch_session_account(
        &self,
        session_id: &str,
        account: &str,
        family: &str,
    ) -> Result<(), ClientError> {
        let body = serde_json::json!({ "account": account, "family": family });
        self.unit(
            Self::route("post_sessions_by_id_switch_account")?,
            &[("id", session_id)],
            Some(&body),
        )
        .await
    }

    /// Enrolled dispatchers with their liveness.
    pub async fn dispatchers(&self) -> Result<Vec<Dispatcher>, ClientError> {
        self.json(Self::route("get_dispatchers")?, &[], &[], None).await
    }

    /// Enroll a dispatcher. The reply carries the key ONCE; it is never stored
    /// server-side beyond a hash, so a caller that loses it must re-enroll.
    pub async fn enroll_dispatcher(
        &self,
        request: &EnrollDispatcher,
    ) -> Result<EnrolledDispatcher, ClientError> {
        let route = Self::route("post_dispatcher_enroll")?;
        let body = serde_json::to_value(request)
            .map_err(|source| ClientError::Decode { route: route.id, source })?;
        self.json(route, &[], &[], Some(&body)).await
    }

    /// Rename a dispatcher or rebind its default account/pool.
    pub async fn update_dispatcher(
        &self,
        id: &str,
        request: &UpdateDispatcher,
    ) -> Result<Dispatcher, ClientError> {
        let route = Self::route("patch_dispatchers_by_id")?;
        let body = serde_json::to_value(request)
            .map_err(|source| ClientError::Decode { route: route.id, source })?;
        self.json(route, &[("id", id)], &[], Some(&body)).await
    }

    pub async fn delete_dispatcher(&self, id: &str) -> Result<(), ClientError> {
        self.unit(Self::route("delete_dispatchers_by_id")?, &[("id", id)], None).await
    }

    // --- instance status and the server's own self-update ---

    /// Server version, build and deployment state. The deployment fields are
    /// only filled in for an authenticated caller.
    pub async fn version(&self) -> Result<crate::instance::VersionInfo, ClientError> {
        self.json(Self::route("get_version")?, &[], &[], None).await
    }

    /// Release notes of every upstream release newer than this server.
    pub async fn version_changelog(&self) -> Result<crate::instance::Changelog, ClientError> {
        self.json(Self::route("get_version_changelog")?, &[], &[], None).await
    }

    /// Probe upstream now instead of waiting out the background interval.
    pub async fn refresh_version(&self) -> Result<(), ClientError> {
        self.unit(Self::route("post_version_refresh")?, &[], None).await
    }

    /// Launch the deployment's self-update. Admin-scoped server-side.
    pub async fn self_update(&self) -> Result<crate::instance::SelfUpdateLaunch, ClientError> {
        self.json(Self::route("post_version_self_update")?, &[], &[], None).await
    }

    /// The most recent update-hook run, or `None` when the deployment has never
    /// run one.
    pub async fn self_update_status(
        &self,
    ) -> Result<Option<crate::instance::SelfUpdateRun>, ClientError> {
        self.json(Self::route("get_version_self_update")?, &[], &[], None).await
    }

    // --- accounts, pools and redirects ---

    /// The caller's account identities with their provider credentials.
    pub async fn accounts(&self) -> Result<Vec<crate::accounts::Account>, ClientError> {
        self.json(Self::route("get_accounts")?, &[], &[], None).await
    }

    /// Rename, re-glyph, or change an account's pool eligibility/weight.
    pub async fn update_account(
        &self,
        id: &str,
        request: &crate::accounts::UpdateAccount,
    ) -> Result<(), ClientError> {
        let route = Self::route("patch_accounts_by_id")?;
        let body = serde_json::to_value(request)
            .map_err(|source| ClientError::Decode { route: route.id, source })?;
        self.unit(route, &[("id", id)], Some(&body)).await
    }

    /// Claim a usage-limit reset. `provider_id` is the provider-row id, not the
    /// account's.
    pub async fn limit_reset(
        &self,
        provider_id: &str,
        request: &crate::accounts::LimitResetRequest,
    ) -> Result<crate::accounts::LimitResetOutcome, ClientError> {
        let route = Self::route("post_accounts_by_id_limit_reset")?;
        let body = serde_json::to_value(request)
            .map_err(|source| ClientError::Decode { route: route.id, source })?;
        self.json(route, &[("id", provider_id)], &[], Some(&body)).await
    }

    /// The caller's live redirect rules.
    pub async fn redirects(&self) -> Result<Vec<crate::accounts::AccountRedirect>, ClientError> {
        self.json(Self::route("get_redirects")?, &[], &[], None).await
    }

    /// Point this account's launches at another account for one family.
    pub async fn put_account_redirect(
        &self,
        id: &str,
        request: &crate::accounts::PutRedirect,
    ) -> Result<(), ClientError> {
        let route = Self::route("put_accounts_by_id_redirect")?;
        let body = serde_json::to_value(request)
            .map_err(|source| ClientError::Decode { route: route.id, source })?;
        self.unit(route, &[("id", id)], Some(&body)).await
    }

    pub async fn delete_redirect(&self, id: &str) -> Result<(), ClientError> {
        self.unit(Self::route("delete_redirects_by_id")?, &[("id", id)], None).await
    }

    /// The caller's account pools with their membership.
    pub async fn account_pools(&self) -> Result<Vec<crate::accounts::AccountPool>, ClientError> {
        self.json(Self::route("get_account_pools")?, &[], &[], None).await
    }

    pub async fn create_account_pool(
        &self,
        request: &crate::accounts::CreatePool,
    ) -> Result<crate::accounts::AccountPool, ClientError> {
        let route = Self::route("post_account_pools")?;
        let body = serde_json::to_value(request)
            .map_err(|source| ClientError::Decode { route: route.id, source })?;
        self.json(route, &[], &[], Some(&body)).await
    }

    pub async fn update_account_pool(
        &self,
        id: &str,
        request: &crate::accounts::UpdatePool,
    ) -> Result<(), ClientError> {
        let route = Self::route("patch_account_pools_by_id")?;
        let body = serde_json::to_value(request)
            .map_err(|source| ClientError::Decode { route: route.id, source })?;
        self.unit(route, &[("id", id)], Some(&body)).await
    }

    pub async fn delete_account_pool(&self, id: &str) -> Result<(), ClientError> {
        self.unit(Self::route("delete_account_pools_by_id")?, &[("id", id)], None).await
    }

    /// Every label the caller owns.
    pub async fn labels(&self) -> Result<Vec<Label>, ClientError> {
        let resp: LabelListResponse = self.json(Self::route("get_labels")?, &[], &[], None).await?;
        Ok(resp.labels)
    }

    /// Get-or-create by name: the server returns the existing label when the
    /// name is taken, so a duplicate create is not an error.
    pub async fn create_label(&self, name: &str, color: &str) -> Result<Label, ClientError> {
        let route = Self::route("post_labels")?;
        let body = serde_json::to_value(CreateLabelRequest {
            name: name.to_owned(),
            color: color.to_owned(),
        })
        .map_err(|source| ClientError::Decode { route: route.id, source })?;
        self.json(route, &[], &[], Some(&body)).await
    }

    /// Rename or recolor; an omitted field is left alone.
    pub async fn update_label(
        &self,
        id: &str,
        name: Option<String>,
        color: Option<String>,
    ) -> Result<Label, ClientError> {
        let route = Self::route("patch_labels_by_id")?;
        let body = serde_json::to_value(UpdateLabelRequest { name, color })
            .map_err(|source| ClientError::Decode { route: route.id, source })?;
        self.json(route, &[("id", id)], &[], Some(&body)).await
    }

    pub async fn delete_label(&self, id: &str) -> Result<(), ClientError> {
        self.unit(Self::route("delete_labels_by_id")?, &[("id", id)], None).await
    }

    pub async fn attach_label(&self, session_id: &str, label_id: &str) -> Result<(), ClientError> {
        let route = Self::route("post_sessions_by_id_labels")?;
        let body = serde_json::to_value(AttachLabelRequest { label_id: label_id.to_owned() })
            .map_err(|source| ClientError::Decode { route: route.id, source })?;
        self.unit(route, &[("id", session_id)], Some(&body)).await
    }

    pub async fn detach_label(&self, session_id: &str, label_id: &str) -> Result<(), ClientError> {
        self.unit(
            Self::route("delete_sessions_by_id_labels_by_label")?,
            &[("id", session_id), ("label_id", label_id)],
            None,
        )
        .await
    }

    pub async fn set_auto_approve(
        &self,
        session_id: &str,
        enabled: bool,
    ) -> Result<(), ClientError> {
        let route = Self::route("post_sessions_by_id_auto_approve")?;
        let body = serde_json::to_value(AutoApproveRequest { enabled })
            .map_err(|source| ClientError::Decode { route: route.id, source })?;
        self.unit(route, &[("id", session_id)], Some(&body)).await
    }

    pub async fn rename_session(&self, session_id: &str, name: &str) -> Result<(), ClientError> {
        let route = Self::route("patch_sessions_by_id")?;
        let body = serde_json::to_value(RenameRequest { name: name.to_owned() })
            .map_err(|source| ClientError::Decode { route: route.id, source })?;
        self.unit(route, &[("id", session_id)], Some(&body)).await
    }

    pub async fn kill_session(&self, session_id: &str) -> Result<(), ClientError> {
        self.unit(Self::route("post_sessions_by_id_kill")?, &[("id", session_id)], None).await
    }

    /// Archives or unarchives a batch of sessions in one request. The server
    /// filters the ids to the ones the caller owns and is idempotent per id.
    pub async fn archive_sessions(
        &self,
        ids: &[String],
        archived: bool,
    ) -> Result<(), ClientError> {
        let id = if archived { "post_sessions_archive" } else { "post_sessions_unarchive" };
        self.unit(Self::route(id)?, &[], Some(&batch_ids(ids))).await
    }

    pub async fn pin_sessions(&self, ids: &[String], pinned: bool) -> Result<(), ClientError> {
        let id = if pinned { "post_sessions_pin" } else { "post_sessions_unpin" };
        self.unit(Self::route(id)?, &[], Some(&batch_ids(ids))).await
    }

    /// `POST /sessions/spawn`. The route is `multipart/form-data`: the JSON
    /// goes in a `request` part, and each attachment in a part of its own.
    pub async fn spawn_session(
        &self,
        request: &cctui_proto::api::SpawnRequest,
        files: Vec<UploadFile>,
    ) -> Result<cctui_proto::api::SpawnResponse, ClientError> {
        let route = Self::route("post_sessions_spawn")?;
        let json = serde_json::to_string(request)
            .map_err(|source| ClientError::Decode { route: route.id, source })?;
        let mut form = reqwest::multipart::Form::new().text("request", json);
        for file in files {
            let part = reqwest::multipart::Part::bytes(file.bytes).file_name(file.name);
            form = form.part("files", part);
        }
        let resp = self
            .http
            .post(self.url_for(route, &[]))
            .bearer_auth(&self.token)
            .multipart(form)
            .timeout(UPLOAD_TIMEOUT)
            .send()
            .await
            .map_err(|source| ClientError::Transport { route: route.id, source })?;
        decode_body(route.id, check_status(route.id, resp).await?).await
    }

    /// The caller's settings blob. The TUI reads it and never writes it back.
    pub async fn settings(&self) -> Result<SettingsPayload, ClientError> {
        self.json(Self::route("get_settings")?, &[], &[], None).await
    }

    /// Requests raised before this client connected, so a card still appears.
    pub async fn pending_permissions(&self) -> Result<Vec<PendingPermissionItem>, ClientError> {
        self.json(Self::route("get_permissions_pending")?, &[], &[], None).await
    }

    pub async fn mark_seen(&self, session_id: &str) -> Result<(), ClientError> {
        self.unit(Self::route("post_sessions_by_id_seen")?, &[("id", session_id)], None).await
    }

    /// Stage files into a live session's working dir, returning the absolute
    /// path each one was written to, in request order. `post_sessions_by_id_files`
    /// is multipart, and the server renames a clash, so the reply — not the
    /// request — says what a `[name]` token must point at.
    pub async fn stage_session_files(
        &self,
        session_id: &str,
        files: Vec<UploadFile>,
    ) -> Result<StageFilesResponse, ClientError> {
        let route = Self::route("post_sessions_by_id_files")?;
        let mut form = reqwest::multipart::Form::new();
        for file in files {
            let part = reqwest::multipart::Part::bytes(file.bytes).file_name(file.name.clone());
            form = form.part("files", part);
        }
        let resp = self
            .http
            .post(self.url_for(route, &[("id", session_id)]))
            .bearer_auth(&self.token)
            .multipart(form)
            .timeout(UPLOAD_TIMEOUT)
            .send()
            .await
            .map_err(|source| ClientError::Transport { route: route.id, source })?;
        decode_body(route.id, check_status(route.id, resp).await?).await
    }

    /// Read one agent-linked file on a machine. Refusals come back as data, not
    /// as an error: the viewer words 403 and 404 differently, and a 403 carries
    /// the roots the path was checked against.
    pub async fn read_machine_file(
        &self,
        machine_id: &str,
        path: &str,
        session_id: &str,
    ) -> Result<FileRead, ClientError> {
        let route = Self::route("get_machines_by_machine_fs_file")?;
        let resp = self
            .http
            .get(self.url_for(route, &[("machine_id", machine_id)]))
            .bearer_auth(&self.token)
            .query(&[("path", path), ("session_id", session_id)])
            .send()
            .await;
        // Status 0 is the viewer's "network" wording, matching the webui.
        let Ok(resp) = resp else { return Ok(FileRead::Refused(FileRefusal::network())) };
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Ok(FileRead::Refused(FileRefusal::parse(status.as_u16(), &body)));
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let bytes = resp
            .bytes()
            .await
            .map_err(|source| ClientError::Transport { route: route.id, source })?
            .to_vec();
        Ok(FileRead::Ok { content_type, bytes })
    }

    /// Any route that answers with a binary body. Refusals come back as data,
    /// like `read_machine_file`: a viewer words a missing blob and a dead
    /// network differently.
    pub async fn get_blob(
        &self,
        route_id: &str,
        params: &[(&str, &str)],
        fallback_type: &str,
    ) -> Result<FileRead, ClientError> {
        let route = Self::route(route_id)?;
        let resp = self.http.get(self.url_for(route, params)).bearer_auth(&self.token).send().await;
        let Ok(resp) = resp else { return Ok(FileRead::Refused(FileRefusal::network())) };
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Ok(FileRead::Refused(FileRefusal::parse(status.as_u16(), &body)));
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(fallback_type)
            .to_owned();
        let bytes = resp
            .bytes()
            .await
            .map_err(|source| ClientError::Transport { route: route.id, source })?
            .to_vec();
        Ok(FileRead::Ok { content_type, bytes })
    }

    /// Which session and machine linked `path`, when this session did not.
    /// `None` when nobody the caller can read did.
    pub async fn linked_file_owner(
        &self,
        session_id: &str,
        path: &str,
    ) -> Result<Option<LinkedFileOwner>, ClientError> {
        let route = Self::route("get_sessions_by_id_linked_file_owner")?;
        match self
            .json::<LinkedFileOwner>(
                route,
                &[("id", session_id)],
                &[("path", path.to_owned())],
                None,
            )
            .await
        {
            Ok(owner) => Ok(Some(owner)),
            Err(ClientError::NotFound { .. } | ClientError::Forbidden { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Every draft the caller owns, in one call.
    pub async fn list_drafts(&self) -> Result<DraftList, ClientError> {
        self.json(Self::route("get_drafts")?, &[], &[], None).await
    }

    /// One draft's text, `None` when the caller has no such draft.
    pub async fn get_draft(&self, key: &str) -> Result<Option<String>, ClientError> {
        let route = Self::route("get_drafts_by_*key")?;
        match self.json::<Draft>(route, &[("*key", key)], &[], None).await {
            Ok(draft) => Ok(Some(draft.text)),
            Err(ClientError::NotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Save a draft. Empty text deletes the row, as the route documents.
    /// Replaces the settings blob. `PUT /settings` is a replace, so the caller
    /// must send the whole blob it read, patched — never just its own keys.
    pub async fn put_settings(
        &self,
        version: i32,
        data: Value,
    ) -> Result<SettingsPayload, ClientError> {
        let route = Self::route("put_settings")?;
        let body = serde_json::to_value(SettingsPayload { version, data })
            .map_err(|source| ClientError::Decode { route: route.id, source })?;
        self.json(route, &[], &[], Some(&body)).await
    }

    /// Full-text session search. `q` is the raw query: the server parses it with
    /// the same `cctui-query` grammar the TUI uses to complete it.
    pub async fn search_sessions(
        &self,
        q: &str,
        include_archived: bool,
        limit: i64,
        offset: i64,
    ) -> Result<SessionListResponse, ClientError> {
        let query = vec![
            ("q", q.to_owned()),
            ("include_archived", include_archived.to_string()),
            ("limit", limit.to_string()),
            ("offset", offset.to_string()),
        ];
        self.json(Self::route("get_sessions_search")?, &[], &query, None).await
    }

    /// Autocomplete values for one search field.
    pub async fn search_values(&self, field: &str, q: &str) -> Result<Vec<String>, ClientError> {
        let query = vec![("field", field.to_owned()), ("q", q.to_owned())];
        self.json(Self::route("get_sessions_search_values")?, &[], &query, None).await
    }

    /// Directories under `path` on a machine, for the spawn dir picker.
    pub async fn machine_dirs(
        &self,
        machine_id: &str,
        path: &str,
    ) -> Result<Vec<String>, ClientError> {
        let route = Self::route("get_machines_by_machine_fs_dirs")?;
        let query = vec![("path", path.to_owned())];
        self.json(route, &[("machine_id", machine_id)], &query, None).await
    }

    /// Branch / detached HEAD / worktree of a directory on a machine.
    pub async fn machine_git_info(
        &self,
        machine_id: &str,
        path: &str,
    ) -> Result<cctui_proto::git::GitInfo, ClientError> {
        let route = Self::route("get_machines_by_machine_fs_gitinfo")?;
        let query = vec![("path", path.to_owned())];
        self.json(route, &[("machine_id", machine_id)], &query, None).await
    }

    /// Re-reads every `OpenAI` account's catalog from upstream.
    pub async fn refresh_codex_models(&self, machine_id: &str) -> Result<(), ClientError> {
        let route = Self::route("post_machines_by_machine_codex_models_refresh")?;
        self.unit(route, &[("machine_id", machine_id)], None).await
    }

    /// Working dirs the caller spawned into recently.
    pub async fn recent_dirs(&self) -> Result<Vec<String>, ClientError> {
        self.json(Self::route("get_sessions_recent_dirs")?, &[], &[], None).await
    }

    /// Every remembered spawn configuration, keyed by target.
    pub async fn spawn_memory(
        &self,
    ) -> Result<cctui_proto::drafts::SpawnMemoryPayload, ClientError> {
        self.json(Self::route("get_spawn_memory")?, &[], &[], None).await
    }

    /// Replaces the whole map; the server caps it.
    pub async fn put_spawn_memory(
        &self,
        payload: &cctui_proto::drafts::SpawnMemoryPayload,
    ) -> Result<cctui_proto::drafts::SpawnMemoryPayload, ClientError> {
        let route = Self::route("put_spawn_memory")?;
        let body = serde_json::to_value(payload)
            .map_err(|source| ClientError::Decode { route: route.id, source })?;
        self.json(route, &[], &[], Some(&body)).await
    }

    pub async fn put_draft(&self, key: &str, text: &str) -> Result<(), ClientError> {
        let route = Self::route("put_drafts_by_*key")?;
        let body = serde_json::to_value(PutDraftRequest { text: text.to_owned() })
            .map_err(|source| ClientError::Decode { route: route.id, source })?;
        self.unit(route, &[("*key", key)], Some(&body)).await
    }

    pub async fn delete_draft(&self, key: &str) -> Result<(), ClientError> {
        self.unit(Self::route("delete_drafts_by_*key")?, &[("*key", key)], None).await
    }

    /// The caller's pinned messages in a session, oldest `seq` first.
    pub async fn list_pins(&self, session_id: &str) -> Result<Vec<MessagePin>, ClientError> {
        self.json(Self::route("get_sessions_by_id_pins")?, &[("id", session_id)], &[], None).await
    }

    /// Pin a message by its stream `seq`.
    pub async fn pin_message(&self, session_id: &str, seq: i64) -> Result<MessagePin, ClientError> {
        let body = serde_json::json!({ "seq": seq, "message_id": Value::Null });
        self.json(Self::route("post_sessions_by_id_pins")?, &[("id", session_id)], &[], Some(&body))
            .await
    }

    pub async fn unpin_message(&self, session_id: &str, seq: i64) -> Result<(), ClientError> {
        let route = Self::route("delete_sessions_by_id_pins_by_seq")?;
        self.unit(route, &[("id", session_id), ("seq", &seq.to_string())], None).await
    }

    /// A page of the caller's bookmarks, newest first. `before` is the
    /// `created_at` of the last row of the previous page.
    pub async fn list_bookmarks(
        &self,
        q: &str,
        before: Option<chrono::DateTime<chrono::Utc>>,
        limit: i64,
    ) -> Result<Vec<Bookmark>, ClientError> {
        let mut query = vec![("limit", limit.to_string())];
        if !q.trim().is_empty() {
            query.push(("q", q.to_owned()));
        }
        if let Some(before) = before {
            query.push(("before", before.to_rfc3339()));
        }
        self.json(Self::route("get_bookmarks")?, &[], &query, None).await
    }

    /// Save a message as a bookmark. The body is a snapshot, so the row
    /// outlives both the message and its session.
    pub async fn create_bookmark(&self, draft: &CreateBookmark) -> Result<Bookmark, ClientError> {
        let route = Self::route("post_bookmarks")?;
        let body = serde_json::to_value(draft)
            .map_err(|source| ClientError::Decode { route: route.id, source })?;
        self.json(route, &[], &[], Some(&body)).await
    }

    /// Edit a bookmark's title and note; the snapshot itself is immutable.
    pub async fn update_bookmark(
        &self,
        id: &str,
        title: &str,
        note: Option<&str>,
    ) -> Result<Bookmark, ClientError> {
        let body = serde_json::json!({ "title": title, "note": note });
        self.json(Self::route("patch_bookmarks_by_id")?, &[("id", id)], &[], Some(&body)).await
    }

    pub async fn delete_bookmark(&self, id: &str) -> Result<(), ClientError> {
        self.unit(Self::route("delete_bookmarks_by_id")?, &[("id", id)], None).await
    }

    /// The caller's spawn profiles, in their stored order.
    pub async fn profiles(&self) -> Result<Vec<SessionProfile>, ClientError> {
        self.json(Self::route("get_profiles")?, &[], &[], None).await
    }

    pub async fn create_profile(
        &self,
        body: &CreateProfileRequest,
    ) -> Result<SessionProfile, ClientError> {
        let route = Self::route("post_profiles")?;
        self.json(route, &[], &[], Some(&to_value(route.id, body)?)).await
    }

    pub async fn update_profile(
        &self,
        id: &str,
        body: &UpdateProfileRequest,
    ) -> Result<SessionProfile, ClientError> {
        let route = Self::route("patch_profiles_by_id")?;
        self.json(route, &[("id", id)], &[], Some(&to_value(route.id, body)?)).await
    }

    pub async fn delete_profile(&self, id: &str) -> Result<(), ClientError> {
        self.unit(Self::route("delete_profiles_by_id")?, &[("id", id)], None).await
    }

    /// Store a new order; the server answers with the reordered list.
    pub async fn reorder_profiles(
        &self,
        ids: Vec<uuid::Uuid>,
    ) -> Result<Vec<SessionProfile>, ClientError> {
        let route = Self::route("put_profiles_order")?;
        let body = to_value(route.id, &ReorderProfilesRequest { ids })?;
        self.json(route, &[], &[], Some(&body)).await
    }

    /// Replace a draft session's stored payload — the autosave and the edit.
    pub async fn update_draft(
        &self,
        session_id: &str,
        body: &SpawnRequest,
    ) -> Result<SpawnResponse, ClientError> {
        let route = Self::route("put_sessions_by_id_draft")?;
        self.json(route, &[("id", session_id)], &[], Some(&to_value(route.id, body)?)).await
    }

    /// Launch a draft. `env` is entered at launch and never stored in the draft.
    pub async fn launch_draft(
        &self,
        session_id: &str,
        env: &std::collections::BTreeMap<String, String>,
    ) -> Result<SpawnResponse, ClientError> {
        let route = Self::route("post_sessions_by_id_launch")?;
        let body = serde_json::json!({ "env": env });
        self.json(route, &[("id", session_id)], &[], Some(&body)).await
    }

    pub async fn discard_draft(&self, session_id: &str) -> Result<(), ClientError> {
        let route = Self::route("post_sessions_by_id_discard")?;
        self.unit(route, &[("id", session_id)], Some(&serde_json::json!({}))).await
    }

    /// Revoke the key this client authenticates with (`cctui logout --revoke`).
    pub async fn revoke_current_key(&self) -> Result<(), ClientError> {
        self.unit(Self::route("delete_me_key")?, &[], None).await
    }
}

/// The `{ids: [...]}` body every batch session route takes.
fn batch_ids(ids: &[String]) -> Value {
    serde_json::json!({ "ids": ids })
}

/// A typed request body as JSON, with the route named in the error.
fn to_value<B: Serialize>(route: &'static str, body: &B) -> Result<Value, ClientError> {
    serde_json::to_value(body).map_err(|source| ClientError::Decode { route, source })
}

fn read_etag(resp: &reqwest::Response) -> Option<String> {
    resp.headers()
        .get(ETAG)
        .or_else(|| resp.headers().get(ETAG_MIRROR))
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

async fn check_status(
    route: &'static str,
    resp: reqwest::Response,
) -> Result<reqwest::Response, ClientError> {
    let status = resp.status();
    if status.is_success() || status == StatusCode::NOT_MODIFIED {
        return Ok(resp);
    }
    match status {
        StatusCode::UNAUTHORIZED => Err(ClientError::Unauthorized),
        StatusCode::FORBIDDEN => Err(ClientError::Forbidden { route }),
        StatusCode::NOT_FOUND => Err(ClientError::NotFound { route }),
        _ => {
            let body = resp.text().await.unwrap_or_default();
            Err(ClientError::Status { route, status: status.as_u16(), body: truncate(&body) })
        }
    }
}

/// An empty body decodes as `null`, so routes that answer `204` still work.
async fn decode_body<R: DeserializeOwned>(
    route: &'static str,
    resp: reqwest::Response,
) -> Result<R, ClientError> {
    let bytes = resp.bytes().await.map_err(|source| ClientError::Transport { route, source })?;
    let slice: &[u8] = if bytes.is_empty() { b"null" } else { &bytes };
    serde_json::from_slice(slice).map_err(|source| ClientError::Decode { route, source })
}

fn truncate(body: &str) -> String {
    const MAX: usize = 400;
    body.char_indices()
        .nth(MAX)
        .map_or_else(|| body.to_owned(), |(cut, _)| format!("{}…", &body[..cut]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client(base_url: &str, token: &str) -> Client {
        let _ = rustls::crypto::ring::default_provider().install_default();
        Client::new(base_url, token)
    }

    fn client() -> Client {
        test_client("http://localhost:8700/", "tok")
    }

    #[test]
    fn urls_come_from_the_route_table() {
        let c = client();
        assert_eq!(
            c.url_for(Client::route("get_sessions").unwrap(), &[]),
            "http://localhost:8700/api/v1/sessions"
        );
        assert_eq!(
            c.url_for(
                Client::route("get_sessions_by_id_conversation").unwrap(),
                &[("id", "abc-123")]
            ),
            "http://localhost:8700/api/v1/sessions/abc-123/conversation"
        );
        assert_eq!(
            c.url_for(Client::route("post_sessions_by_id_seen").unwrap(), &[("id", "s1")]),
            "http://localhost:8700/api/v1/sessions/s1/seen"
        );
        assert_eq!(
            c.url_for(Client::route("get_me").unwrap(), &[]),
            "http://localhost:8700/api/v1/me"
        );
    }

    /// The draft routes take a wildcard segment, so their placeholder is
    /// `{*key}` and the param name carries the star.
    #[test]
    fn a_draft_key_fills_the_wildcard_segment() {
        let c = client();
        let put = Client::route("put_drafts_by_*key").unwrap();
        assert_eq!(
            c.url_for(put, &[("*key", "cctui_draft_s1")]),
            "http://localhost:8700/api/v1/drafts/cctui_draft_s1"
        );
        let get = Client::route("get_drafts_by_*key").unwrap();
        assert_eq!(
            c.url_for(get, &[("*key", "cctui_history_s1")]),
            "http://localhost:8700/api/v1/drafts/cctui_history_s1"
        );
        assert_eq!(
            c.url_for(Client::route("get_drafts").unwrap(), &[]),
            "http://localhost:8700/api/v1/drafts"
        );
    }

    #[test]
    fn every_named_route_exists_in_the_table() {
        for id in [
            "get_sessions",
            "get_sessions_by_id_bindings",
            "post_sessions_by_id_switch_account",
            "get_sessions_search",
            "get_sessions_search_values",
            "put_settings",
            "post_sessions_spawn",
            "get_machines_by_machine_fs_dirs",
            "get_machines_by_machine_fs_gitinfo",
            "get_sessions_recent_dirs",
            "post_machines_by_machine_codex_models_refresh",
            "get_spawn_memory",
            "put_spawn_memory",
            "get_sessions_by_id",
            "get_sessions_stats",
            "get_sessions_by_id_conversation",
            "get_sessions_by_id_diagnose",
            "post_sessions_by_id_interrupt",
            "post_sessions_by_id_set_model",
            "post_sessions_by_id_fork",
            "post_sessions_by_id_resume",
            "get_sessions_dispatchers",
            "post_sessions_dispatch",
            "get_models_by_harness",
            "post_sessions_by_id_auto_approve",
            "post_sessions_by_id_seen",
            "get_permissions_pending",
            "get_labels",
            "get_machines_resources",
            "get_sessions_stats_tokens",
            "get_sessions_stats_usage",
            "get_sessions_stats_cache_busts",
            "get_sessions_by_id_langfuse",
            "get_accounts_usage",
            "get_account_pools_usage",
            "get_dispatchers",
            "post_dispatcher_enroll",
            "patch_dispatchers_by_id",
            "delete_dispatchers_by_id",
            "post_labels",
            "patch_labels_by_id",
            "delete_labels_by_id",
            "post_sessions_by_id_labels",
            "delete_sessions_by_id_labels_by_label",
            "post_sessions_by_id_files",
            "get_machines_by_machine_fs_file",
            "get_sessions_by_id_linked_file_owner",
            "get_me",
            "get_settings",
            "delete_me_key",
            "get_drafts",
            "get_drafts_by_*key",
            "put_drafts_by_*key",
            "delete_drafts_by_*key",
            "get_accounts",
            "get_account_pools",
            "patch_sessions_by_id",
            "post_sessions_by_id_kill",
            "post_sessions_archive",
            "post_sessions_unarchive",
            "post_sessions_pin",
            "post_sessions_unpin",
            "get_sessions_by_id_pins",
            "post_sessions_by_id_pins",
            "delete_sessions_by_id_pins_by_seq",
            "get_bookmarks",
            "post_bookmarks",
            "get_profiles",
            "post_profiles",
            "patch_profiles_by_id",
            "delete_profiles_by_id",
            "put_profiles_order",
            "put_sessions_by_id_draft",
            "post_sessions_by_id_launch",
            "post_sessions_by_id_discard",
            "patch_bookmarks_by_id",
            "delete_bookmarks_by_id",
        ] {
            assert!(Client::route(id).is_ok(), "missing route id {id}");
        }
    }

    #[test]
    fn unknown_route_id_is_a_typed_error() {
        let err = Client::route("get_nope").unwrap_err();
        assert!(matches!(err, ClientError::UnknownRoute(id) if id == "get_nope"));
    }

    #[test]
    fn ws_url_upgrades_the_scheme_and_keeps_the_api_prefix() {
        assert_eq!(client().ws_url(), "ws://localhost:8700/api/v1/ws");
        assert_eq!(
            test_client("https://cctui.example.com", "t").ws_url(),
            "wss://cctui.example.com/api/v1/ws"
        );
    }

    #[test]
    fn page_query_omits_absent_bounds() {
        assert!(Page::default().query().is_empty());
        assert_eq!(Page::latest(50).query(), vec![("limit", "50".to_owned())]);
        assert_eq!(
            Page::before(900, 50).query(),
            vec![("before", "900".to_owned()), ("limit", "50".to_owned())]
        );
        assert_eq!(Page::after(12).query(), vec![("after", "12".to_owned())]);
    }

    #[test]
    fn conversation_row_keeps_the_whole_event() {
        let row: ConversationRow = serde_json::from_value(serde_json::json!({
            "type": "assistant",
            "seq": 42,
            "ts": 1_700_000_000_000_i64,
            "turn_id": "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000",
            "content": "hi",
        }))
        .unwrap();
        assert_eq!(row.seq, 42);
        assert_eq!(row.ts, Some(1_700_000_000_000));
        assert!(row.turn_id.is_some());
        assert_eq!(row.event.get("content").unwrap(), "hi");
        assert_eq!(row.event.get("seq").unwrap(), 42);
    }

    #[test]
    fn conversation_row_without_seq_defaults_to_zero() {
        let row: ConversationRow =
            serde_json::from_value(serde_json::json!({ "type": "system" })).unwrap();
        assert_eq!(row.seq, 0);
        assert_eq!(row.ts, None);
        assert_eq!(row.turn_id, None);
    }

    /// A listener that accepts and then says nothing, standing in for a server
    /// that has stopped answering while the socket stays up.
    fn hung_server() -> (String, std::net::TcpListener) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        let port = listener.local_addr().expect("an address").port();
        (format!("http://127.0.0.1:{port}"), listener)
    }

    #[tokio::test]
    async fn a_server_that_accepts_and_never_answers_is_given_up_on() {
        let (base, listener) = hung_server();
        std::thread::spawn(move || {
            let held = listener.accept();
            std::thread::sleep(std::time::Duration::from_secs(30));
            drop(held);
        });

        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = Client::with_timeouts(
            &base,
            "tok",
            std::time::Duration::from_millis(500),
            std::time::Duration::from_millis(300),
        );
        let started = std::time::Instant::now();
        let outcome =
            tokio::time::timeout(std::time::Duration::from_secs(10), client.list_sessions()).await;

        let result = outcome.expect("the request has a deadline of its own and must not hang");
        assert!(result.is_err(), "a server that never answers cannot produce a session list");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "it gave up after {:?}, which is not a deadline",
            started.elapsed()
        );
    }

    #[test]
    fn the_default_bounds_are_finite_and_ordered() {
        assert!(CONNECT_TIMEOUT < REQUEST_TIMEOUT, "connecting is the tight bound");
        assert!(
            REQUEST_TIMEOUT < UPLOAD_TIMEOUT,
            "an upload pushes bytes, so it gets longer than a plain request"
        );
    }

    #[test]
    fn base_url_trailing_slash_does_not_double_up() {
        assert_eq!(test_client("http://h:1/", "t").base_url(), "http://h:1");
        assert_eq!(test_client("http://h:1", "t").base_url(), "http://h:1");
    }
}
