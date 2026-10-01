use cctui_proto::api::bookmarks::{Bookmark, CreateBookmark};
use cctui_proto::api::me::MeResponse;
use cctui_proto::api::routes::{Method, Route, by_id};
use cctui_proto::api::settings::SettingsPayload;
use cctui_proto::api::{
    AttachLabelRequest, AutoApproveRequest, CreateLabelRequest, ForkRequest, ForkResponse, Label,
    LabelListResponse, RenameRequest, SessionListItem, SessionListResponse, SessionStats,
    SetModelRequest, StageFilesResponse, UpdateLabelRequest,
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

/// Envoy strips `ETag` off responses it compresses; the server mirrors it here.
const ETAG_MIRROR: &str = "x-etag";

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
        Self {
            base_url: base_url.as_ref().trim_end_matches('/').to_owned(),
            token: token.into(),
            http: reqwest::Client::new(),
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

    /// Fork a whole session. Every field of [`ForkRequest`] is inherited, so
    /// the fork needs no options.
    pub async fn fork(&self, session_id: &str) -> Result<ForkResponse, ClientError> {
        let route = Self::route("post_sessions_by_id_fork")?;
        let body = serde_json::to_value(ForkRequest::default())
            .map_err(|source| ClientError::Decode { route: route.id, source })?;
        self.json(route, &[("id", session_id)], &[], Some(&body)).await
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

    /// `POST /sessions/spawn`. The route is multipart so a spawn can carry file
    /// uploads; with none to send, the JSON body is the only part.
    pub async fn spawn_session(
        &self,
        request: &cctui_proto::api::SpawnRequest,
    ) -> Result<cctui_proto::api::SpawnResponse, ClientError> {
        let route = Self::route("post_sessions_spawn")?;
        let body = serde_json::to_value(request)
            .map_err(|source| ClientError::Decode { route: route.id, source })?;
        self.json(route, &[], &[], Some(&body)).await
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

    /// Revoke the key this client authenticates with (`cctui logout --revoke`).
    pub async fn revoke_current_key(&self) -> Result<(), ClientError> {
        self.unit(Self::route("delete_me_key")?, &[], None).await
    }
}

/// The `{ids: [...]}` body every batch session route takes.
fn batch_ids(ids: &[String]) -> Value {
    serde_json::json!({ "ids": ids })
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
            "get_models_by_harness",
            "post_sessions_by_id_auto_approve",
            "post_sessions_by_id_seen",
            "get_permissions_pending",
            "get_labels",
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

    #[test]
    fn base_url_trailing_slash_does_not_double_up() {
        assert_eq!(test_client("http://h:1/", "t").base_url(), "http://h:1");
        assert_eq!(test_client("http://h:1", "t").base_url(), "http://h:1");
    }
}
