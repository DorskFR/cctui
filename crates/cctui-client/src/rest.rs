use cctui_proto::api::me::MeResponse;
use cctui_proto::api::routes::{Method, Route, by_id};
use cctui_proto::api::settings::SettingsPayload;
use cctui_proto::api::{
    AutoApproveRequest, SessionListItem, SessionListResponse, StageFilesResponse,
};
use cctui_proto::diagnose::SessionDiagnoseResponse;
use cctui_proto::drafts::{Draft, DraftList, PutDraftRequest};
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

    pub async fn interrupt(&self, session_id: &str) -> Result<(), ClientError> {
        self.unit(Self::route("post_sessions_by_id_interrupt")?, &[("id", session_id)], None).await
    }

    pub async fn diagnose(&self, session_id: &str) -> Result<SessionDiagnoseResponse, ClientError> {
        self.json(Self::route("get_sessions_by_id_diagnose")?, &[("id", session_id)], &[], None)
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

    /// Revoke the key this client authenticates with (`cctui logout --revoke`).
    pub async fn revoke_current_key(&self) -> Result<(), ClientError> {
        self.unit(Self::route("delete_me_key")?, &[], None).await
    }
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
            "get_sessions_by_id",
            "get_sessions_by_id_conversation",
            "get_sessions_by_id_diagnose",
            "post_sessions_by_id_interrupt",
            "post_sessions_by_id_auto_approve",
            "post_sessions_by_id_seen",
            "get_permissions_pending",
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
            "get_sessions_by_id_pins",
            "post_sessions_by_id_pins",
            "delete_sessions_by_id_pins_by_seq",
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
