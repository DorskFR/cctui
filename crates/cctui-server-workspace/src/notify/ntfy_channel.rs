use std::sync::{Arc, OnceLock};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use super::channel::{
    Button, Capabilities, Channel, Endpoint, Press, ProviderRef, Rendered, SendError,
};
use crate::config::Config;

pub const ID: &str = "ntfy";

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(crate::build_http_client)
}

pub const SEALED_TOKEN: &str = "token_sealed";
const MAX_TOPIC_CHARS: usize = 64;

pub struct NtfyChannel {
    env: Option<(String, cctui_secrets::Secret)>,
}

struct Target {
    url: String,
    token: Option<Arc<str>>,
    client: &'static reqwest::Client,
}

impl Target {
    fn request(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        let req = self.client.request(method, url);
        let Some(token) = &self.token else { return req };
        req.bearer_auth(token)
    }
}

#[must_use]
pub fn valid_topic(topic: &str) -> bool {
    !topic.is_empty()
        && topic.chars().count() <= MAX_TOPIC_CHARS
        && topic.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `{url}/{topic}` of a user endpoint's `{url, topic}` config.
#[must_use]
pub fn user_topic_url(config: &Value) -> Option<String> {
    let url = config.get("url")?.as_str()?.trim_end_matches('/');
    let topic = config.get("topic")?.as_str()?;
    (!url.is_empty() && valid_topic(topic)).then(|| format!("{url}/{topic}"))
}

fn unseal(sealed: &str) -> Result<Arc<str>, SendError> {
    crate::crypto::decrypt(sealed, &crate::crypto::vault_key())
        .map(Arc::from)
        .ok_or_else(|| SendError::Permanent("the ntfy token could not be unsealed".into()))
}

fn user_target(config: &Value) -> Result<Target, SendError> {
    let url = user_topic_url(config)
        .ok_or_else(|| SendError::Permanent("unsupported ntfy endpoint".into()))?;
    let token = config.get(SEALED_TOKEN).and_then(Value::as_str).map(unseal).transpose()?;
    Ok(Target { url, token, client: crate::outbound::upstream_client() })
}

impl NtfyChannel {
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        let env = config
            .ntfy_url
            .as_deref()
            .zip(config.ntfy_token.clone())
            .map(|(url, token)| (url.trim_end_matches('/').to_owned(), token));
        Self { env }
    }

    #[cfg(test)]
    #[must_use]
    pub fn new(topic_url: &str, token: cctui_secrets::Secret) -> Self {
        Self { env: Some((topic_url.trim_end_matches('/').to_owned(), token)) }
    }

    /// The server token only ever goes to the server's own topic.
    fn target(&self, to: &Endpoint) -> Result<Target, SendError> {
        if to.config.get("source").and_then(Value::as_str) != Some("env") {
            return user_target(&to.config);
        }
        let (url, token) = self.env.as_ref().ok_or_else(|| {
            SendError::Permanent("the server ntfy topic is not configured".into())
        })?;
        Ok(Target { url: url.clone(), token: Some(token.get()), client: client() })
    }
}

async fn publish(n: &Rendered, to: &Target, sequence_id: &str) -> Result<(), SendError> {
    let mut req = to
        .request(reqwest::Method::POST, &to.url)
        .header("X-Sequence-ID", sequence_id)
        .header("Title", encode_header(&n.title))
        .header("Priority", n.priority.to_string());
    if let Some(tags) = &n.tags {
        req = req.header("Tags", encode_header(tags));
    }
    if let Some(link) = &n.link {
        req = req.header("Click", encode_header(link));
    }
    if !n.buttons.is_empty() {
        req = req.header("Actions", encode_header(&actions(&n.buttons).to_string()));
    }
    classify(req.body(n.body.clone().unwrap_or_default()).send().await).await
}

/// ntfy decodes RFC 2047 words; a raw non-ASCII header would fail the request builder.
fn encode_header(value: &str) -> String {
    if value.bytes().all(|b| b == b' ' || b.is_ascii_graphic()) {
        return value.to_owned();
    }
    format!("=?UTF-8?B?{}?=", STANDARD.encode(value))
}

/// The JSON form of ntfy's `Actions` header; a POST also clears the phone notification.
fn actions(buttons: &[Button]) -> Value {
    let all = buttons.iter().map(|b| match b.press {
        Press::Open => json!({ "action": "view", "label": b.label, "url": b.url }),
        Press::Post => json!({
            "action": "http", "label": b.label, "url": b.url, "method": "POST", "clear": true,
        }),
    });
    Value::Array(all.collect())
}

fn sequence_id(r: &ProviderRef) -> Result<&str, SendError> {
    r.0.get("sequence_id")
        .and_then(Value::as_str)
        .ok_or_else(|| SendError::Permanent("ntfy provider ref has no sequence id".into()))
}

async fn classify(resp: Result<reqwest::Response, reqwest::Error>) -> Result<(), SendError> {
    let resp = match resp {
        Ok(resp) => resp,
        Err(e) if e.is_builder() => return Err(SendError::Permanent(e.to_string())),
        Err(e) => return Err(SendError::Retry(e.to_string())),
    };
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    if status == reqwest::StatusCode::GONE {
        return Err(SendError::Gone);
    }
    let detail = format!("ntfy returned {status}: {}", resp.text().await.unwrap_or_default());
    if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        Err(SendError::Retry(detail))
    } else {
        Err(SendError::Permanent(detail))
    }
}

#[async_trait::async_trait]
impl Channel for NtfyChannel {
    fn id(&self) -> &'static str {
        ID
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { update: true, retract: true, buttons: 3, needs_token: true }
    }

    async fn send(&self, n: &Rendered, to: &Endpoint) -> Result<ProviderRef, SendError> {
        let sequence_id = n.delivery_id.to_string();
        publish(n, &self.target(to)?, &sequence_id).await?;
        Ok(ProviderRef(json!({ "sequence_id": sequence_id })))
    }

    async fn update(&self, n: &Rendered, to: &Endpoint, r: &ProviderRef) -> Result<(), SendError> {
        publish(n, &self.target(to)?, sequence_id(r)?).await
    }

    async fn retract(&self, to: &Endpoint, r: &ProviderRef) -> Result<(), SendError> {
        let target = self.target(to)?;
        let url = format!("{}/{}/clear", target.url, sequence_id(r)?);
        classify(target.request(reqwest::Method::PUT, &url).send().await).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::json;

    use super::{NtfyChannel, encode_header};
    use crate::notify::channel::{
        Button, Channel, Endpoint, Press, ProviderRef, Rendered, SendError,
    };

    type Seen = Arc<Mutex<Vec<(String, String, http::HeaderMap)>>>;

    async fn stub(status: u16) -> (String, Seen) {
        let seen: Seen = Arc::default();
        let recorder = seen.clone();
        let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
            let recorder = recorder.clone();
            async move {
                recorder.lock().unwrap().push((
                    req.method().to_string(),
                    req.uri().path().to_owned(),
                    req.headers().clone(),
                ));
                http::StatusCode::from_u16(status).unwrap()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, seen)
    }

    fn channel(base: &str) -> NtfyChannel {
        let url = format!("{base}/cctui/");
        NtfyChannel::new(&url, cctui_secrets::Secret::fixed("ntfy", "tok"))
    }

    fn env() -> Endpoint {
        Endpoint { config: json!({ "source": "env" }) }
    }

    fn rendered() -> Rendered {
        Rendered {
            delivery_id: uuid::Uuid::new_v4(),
            title: "Dispatch FAILED".into(),
            body: Some("worker exploded".into()),
            priority: 5,
            tags: Some("rotating_light".into()),
            link: None,
            buttons: Vec::new(),
        }
    }

    fn header<'a>(headers: &'a http::HeaderMap, name: &str) -> &'a str {
        headers.get(name).unwrap().to_str().unwrap()
    }

    #[tokio::test]
    async fn send_publishes_with_the_delivery_id_as_sequence_id() {
        let (base, seen) = stub(200).await;
        let n = rendered();
        let r = channel(&base).send(&n, &env()).await.unwrap();
        assert_eq!(r, ProviderRef(json!({ "sequence_id": n.delivery_id.to_string() })));

        let (method, path, headers) = seen.lock().unwrap().pop().unwrap();
        assert_eq!((method.as_str(), path.as_str()), ("POST", "/cctui"));
        assert_eq!(header(&headers, "x-sequence-id"), n.delivery_id.to_string());
        assert_eq!(header(&headers, "authorization"), "Bearer tok");
        assert_eq!(header(&headers, "title"), "Dispatch FAILED");
        assert_eq!(header(&headers, "tags"), "rotating_light");
        assert_eq!(header(&headers, "priority"), "5");
    }

    #[tokio::test]
    async fn buttons_go_out_as_view_and_http_actions_that_clear() {
        let (base, seen) = stub(200).await;
        let n = Rendered {
            link: Some("https://cctui.example/sessions/s1".into()),
            buttons: vec![
                Button {
                    label: "Open".into(),
                    url: "https://cctui.example/sessions/s1".into(),
                    press: Press::Open,
                },
                Button {
                    label: "Approve".into(),
                    url: "https://cctui.example/api/v1/notifications/actions/tok".into(),
                    press: Press::Post,
                },
            ],
            ..rendered()
        };
        channel(&base).send(&n, &env()).await.unwrap();

        let (_, _, headers) = seen.lock().unwrap().pop().unwrap();
        assert_eq!(header(&headers, "click"), "https://cctui.example/sessions/s1");
        let actions: serde_json::Value = serde_json::from_str(header(&headers, "actions")).unwrap();
        assert_eq!(
            actions,
            json!([
                { "action": "view", "label": "Open", "url": "https://cctui.example/sessions/s1" },
                {
                    "action": "http",
                    "label": "Approve",
                    "url": "https://cctui.example/api/v1/notifications/actions/tok",
                    "method": "POST",
                    "clear": true,
                },
            ])
        );
    }

    #[tokio::test]
    async fn a_bare_notice_sends_no_click_or_actions() {
        let (base, seen) = stub(200).await;
        channel(&base).send(&rendered(), &env()).await.unwrap();
        let (_, _, headers) = seen.lock().unwrap().pop().unwrap();
        assert!(headers.get("click").is_none());
        assert!(headers.get("actions").is_none());
    }

    #[test]
    fn non_ascii_headers_are_rfc2047_encoded() {
        assert_eq!(encode_header("Dispatch FAILED: x"), "Dispatch FAILED: x");
        assert_eq!(encode_header("é"), "=?UTF-8?B?w6k=?=");
        assert_eq!(encode_header("a\nb"), "=?UTF-8?B?YQpi?=");
    }

    #[tokio::test]
    async fn update_republishes_and_retract_clears_the_same_sequence() {
        let (base, seen) = stub(200).await;
        let ch = channel(&base);
        let r = ProviderRef(json!({ "sequence_id": "seq-1" }));
        ch.update(&rendered(), &env(), &r).await.unwrap();
        ch.retract(&env(), &r).await.unwrap();

        let calls = seen.lock().unwrap().clone();
        let routes: Vec<(&str, &str)> =
            calls.iter().map(|(m, p, _)| (m.as_str(), p.as_str())).collect();
        assert_eq!(routes, [("POST", "/cctui"), ("PUT", "/cctui/seq-1/clear")]);
        assert_eq!(header(&calls[0].2, "x-sequence-id"), "seq-1");
        assert_eq!(header(&calls[1].2, "authorization"), "Bearer tok");
    }

    #[tokio::test]
    async fn status_codes_map_to_retry_permanent_and_gone() {
        for (status, expect_retry, expect_gone) in
            [(503, true, false), (429, true, false), (400, false, false), (410, false, true)]
        {
            let (base, _) = stub(status).await;
            let err = channel(&base).send(&rendered(), &env()).await.unwrap_err();
            assert_eq!(matches!(err, SendError::Retry(_)), expect_retry, "{status}: {err:?}");
            assert_eq!(err == SendError::Gone, expect_gone, "{status}: {err:?}");
        }
    }

    #[tokio::test]
    async fn the_server_token_never_goes_to_a_foreign_endpoint() {
        let (base, seen) = stub(200).await;
        let foreign = Endpoint { config: json!({ "topic_url": "https://elsewhere/t" }) };
        let err = channel(&base).send(&rendered(), &foreign).await.unwrap_err();
        assert!(matches!(err, SendError::Permanent(_)), "{err:?}");
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_user_endpoint_gets_its_own_topic_and_no_server_token() {
        let (base, seen) = stub(200).await;
        let mine = Endpoint { config: json!({ "url": format!("{base}/"), "topic": "mine" }) };
        NtfyChannel { env: None }.send(&rendered(), &mine).await.unwrap();
        let (method, path, headers) = seen.lock().unwrap().pop().unwrap();
        assert_eq!((method.as_str(), path.as_str()), ("POST", "/mine"));
        assert!(headers.get("authorization").is_none(), "the server token stays home");
    }

    #[tokio::test]
    async fn a_user_endpoint_token_is_unsealed_at_send_time() {
        crate::crypto::install_vault_key(vec![0x5a; 32]);
        let (base, seen) = stub(200).await;
        let sealed = crate::crypto::encrypt("utok", &crate::crypto::vault_key());
        let mine = Endpoint {
            config: json!({ "url": base, "topic": "mine", super::SEALED_TOKEN: sealed }),
        };
        let r = channel(&base).send(&rendered(), &mine).await.unwrap();
        channel(&base).retract(&mine, &r).await.unwrap();
        let calls = seen.lock().unwrap().clone();
        assert_eq!(header(&calls[0].2, "authorization"), "Bearer utok");
        assert_eq!(calls[1].0, "PUT");
        assert!(calls[1].1.starts_with("/mine/"), "{}", calls[1].1);
        assert_eq!(header(&calls[1].2, "authorization"), "Bearer utok");
    }

    #[test]
    fn user_topic_urls_need_a_url_and_a_plain_topic() {
        use super::user_topic_url;
        assert_eq!(
            user_topic_url(&json!({ "url": "https://ntfy.example/", "topic": "a_b-1" })).as_deref(),
            Some("https://ntfy.example/a_b-1")
        );
        for bad in [
            json!({ "url": "https://ntfy.example" }),
            json!({ "topic": "t" }),
            json!({ "url": "https://ntfy.example", "topic": "../admin" }),
            json!({ "url": "https://ntfy.example", "topic": "" }),
        ] {
            assert_eq!(user_topic_url(&bad), None, "{bad}");
        }
    }

    async fn db_user(name: &str) -> Option<(sqlx::PgPool, uuid::Uuid)> {
        let url = crate::routes::gateway::test_db_url(name)?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let id = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(format!("ntfy-{id}"))
            .bind(format!("ntfy-key-{id}"))
            .execute(&pool)
            .await
            .unwrap();
        Some((pool, id))
    }

    #[tokio::test]
    async fn a_collapse_republishes_the_sequence_and_resolving_clears_it() {
        use crate::notify::store::{Edge, NewNotification, Routing, emit, transition};
        use crate::notify::{Channels, dispatcher, kind};

        let Some((pool, u)) = db_user("ntfy_update_clear").await else { return };
        let (base, seen) = stub(200).await;
        let mut channels = Channels::default();
        channels.add(Arc::new(channel(&base)));
        let routing = Routing { ntfy: true, env_owner: Some(u) };
        let notice = |title: &str| NewNotification {
            collapse_key: Some(format!("crashloop:{u}")),
            ..NewNotification::new(u, kind::SYSTEM_ALERT, title.into())
        };

        let first = emit(&pool, routing, notice("crashloop x1")).await.unwrap();
        dispatcher::run(&pool, &channels, Some(first.id)).await;
        let second = emit(&pool, routing, notice("crashloop x2")).await.unwrap();
        assert_eq!(second.id, first.id);
        sqlx::query(
            "UPDATE notification_deliveries SET next_attempt_at = now() WHERE notification_id = $1",
        )
        .bind(first.id)
        .execute(&pool)
        .await
        .unwrap();
        dispatcher::run(&pool, &channels, Some(first.id)).await;
        transition(&pool, first.id, Edge::Resolve { reason: "subject_gone" }).await.unwrap();
        dispatcher::run(&pool, &channels, Some(first.id)).await;

        let (delivery, state): (uuid::Uuid, String) = sqlx::query_as(
            "SELECT id, state FROM notification_deliveries WHERE notification_id = $1",
        )
        .bind(first.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(state, "retracted");
        let seq = delivery.to_string();
        let calls = seen.lock().unwrap().clone();
        let routes: Vec<(&str, &str)> =
            calls.iter().map(|(m, p, _)| (m.as_str(), p.as_str())).collect();
        let clear = format!("/cctui/{seq}/clear");
        assert_eq!(routes, [("POST", "/cctui"), ("POST", "/cctui"), ("PUT", clear.as_str())]);
        let titles: Vec<&str> = calls[..2].iter().map(|(_, _, h)| header(h, "title")).collect();
        assert_eq!(titles, ["crashloop x1", "crashloop x2"]);
        assert_eq!(header(&calls[0].2, "x-sequence-id"), seq);
        assert_eq!(header(&calls[1].2, "x-sequence-id"), seq, "the update replaces in place");

        sqlx::query("DELETE FROM users WHERE id = $1").bind(u).execute(&pool).await.unwrap();
    }

    mod pushes {
        use std::sync::Arc;

        use serde_json::{Value, json};
        use sqlx::PgPool;
        use uuid::Uuid;

        use super::{channel, db_user, header, stub};
        use crate::notify::channel::{Channel, Endpoint, ProviderRef, Rendered, SendError};
        use crate::notify::store::{NewNotification, Routing, emit};
        use crate::notify::{Channels, Push, dispatcher, emit_now, is_recorded, kind, tokens};

        const fn on(owner: Uuid) -> Routing {
            Routing { ntfy: true, env_owner: Some(owner) }
        }

        fn ntfy(base: &str) -> Channels {
            let mut channels = Channels::default();
            channels.add(Arc::new(channel(base)));
            channels
        }

        fn crashloop(u: Uuid, title: &str) -> NewNotification {
            NewNotification {
                collapse_key: Some(format!("crashloop:{u}")),
                severity: "high",
                ..NewNotification::new(u, kind::SYSTEM_ALERT, title.into())
            }
        }

        async fn force_due(pool: &PgPool, id: Uuid) {
            sqlx::query(
                "UPDATE notification_deliveries SET next_attempt_at = now() \
                 WHERE notification_id = $1",
            )
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
        }

        /// `(op, state, throttled, stage)`; throttled means due no sooner than ~a minute out.
        async fn delivery(pool: &PgPool, id: Uuid) -> (Option<String>, String, bool, i32) {
            sqlx::query_as(
                "SELECT op, state, next_attempt_at > now() + interval '50 seconds', stage \
                 FROM notification_deliveries WHERE notification_id = $1",
            )
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
        }

        async fn notice_id(pool: &PgPool, u: Uuid) -> (Uuid, i32, String) {
            sqlx::query_as("SELECT id, repeat_count, title FROM notifications WHERE user_id = $1")
                .bind(u)
                .fetch_one(pool)
                .await
                .unwrap()
        }

        async fn drop_user(pool: &PgPool, u: Uuid) {
            sqlx::query("DELETE FROM users WHERE id = $1").bind(u).execute(pool).await.unwrap();
        }

        #[tokio::test]
        async fn two_crashloop_flaps_collapse_into_one_row_and_one_push() {
            let Some((pool, u)) = db_user("ntfy_crashloop_collapse").await else { return };
            let (base, seen) = stub(200).await;
            let channels = ntfy(&base);
            emit_now(&pool, on(u), &channels, crashloop(u, "crashloop x1")).await;
            emit_now(&pool, on(u), &channels, crashloop(u, "crashloop x2")).await;

            let (id, repeats, title) = notice_id(&pool, u).await;
            assert_eq!((repeats, title.as_str()), (2, "crashloop x2"));
            assert_eq!(seen.lock().unwrap().len(), 1, "the second flap does not ping again");
            let (op, state, throttled, _) = delivery(&pool, id).await;
            assert_eq!((op.as_deref(), state.as_str(), throttled), (Some("update"), "sent", true));
            drop_user(&pool, u).await;
        }

        #[tokio::test]
        async fn collapse_updates_wait_out_the_sixty_second_throttle() {
            let Some((pool, u)) = db_user("ntfy_update_throttle").await else { return };
            let (base, seen) = stub(200).await;
            let channels = ntfy(&base);
            emit_now(&pool, on(u), &channels, crashloop(u, "x1")).await;
            emit_now(&pool, on(u), &channels, crashloop(u, "x2")).await;
            let (id, _, _) = notice_id(&pool, u).await;
            dispatcher::run(&pool, &channels, Some(id)).await;
            assert_eq!(seen.lock().unwrap().len(), 1, "the update is held back");

            force_due(&pool, id).await;
            dispatcher::run(&pool, &channels, Some(id)).await;
            emit_now(&pool, on(u), &channels, crashloop(u, "x3")).await;

            let calls = seen.lock().unwrap().clone();
            let titles: Vec<&str> = calls.iter().map(|(_, _, h)| header(h, "title")).collect();
            assert_eq!(titles, ["x1", "x2"], "x3 waits a minute after the x2 update");
            let (op, _, throttled, _) = delivery(&pool, id).await;
            assert_eq!((op.as_deref(), throttled), (Some("update"), true));
            drop_user(&pool, u).await;
        }

        /// Collapses the notice while its send is on the wire.
        struct CollapseMidSend {
            inner: super::NtfyChannel,
            pool: PgPool,
            notice: NewNotification,
        }

        #[async_trait::async_trait]
        impl Channel for CollapseMidSend {
            fn id(&self) -> &'static str {
                self.inner.id()
            }

            fn capabilities(&self) -> crate::notify::channel::Capabilities {
                self.inner.capabilities()
            }

            async fn send(&self, n: &Rendered, to: &Endpoint) -> Result<ProviderRef, SendError> {
                emit(&self.pool, Routing::default(), self.notice.clone()).await.unwrap();
                self.inner.send(n, to).await
            }

            async fn update(
                &self,
                n: &Rendered,
                to: &Endpoint,
                r: &ProviderRef,
            ) -> Result<(), SendError> {
                self.inner.update(n, to, r).await
            }

            async fn retract(&self, to: &Endpoint, r: &ProviderRef) -> Result<(), SendError> {
                self.inner.retract(to, r).await
            }
        }

        #[tokio::test]
        async fn a_collapse_during_an_in_flight_send_still_reaches_the_phone() {
            let Some((pool, u)) = db_user("ntfy_inflight_collapse").await else { return };
            let (base, seen) = stub(200).await;
            let mut racing = Channels::default();
            racing.add(Arc::new(CollapseMidSend {
                inner: channel(&base),
                pool: pool.clone(),
                notice: crashloop(u, "x2"),
            }));
            let first = emit(&pool, on(u), crashloop(u, "x1")).await.unwrap();
            dispatcher::run(&pool, &racing, Some(first.id)).await;

            let (op, state, throttled, _) = delivery(&pool, first.id).await;
            assert_eq!((op.as_deref(), state.as_str(), throttled), (Some("update"), "sent", true));

            force_due(&pool, first.id).await;
            dispatcher::run(&pool, &ntfy(&base), Some(first.id)).await;
            let calls = seen.lock().unwrap().clone();
            let titles: Vec<&str> = calls.iter().map(|(_, _, h)| header(h, "title")).collect();
            assert_eq!(titles, ["x1", "x2"]);
            let seqs: Vec<&str> =
                calls.iter().map(|(_, _, h)| header(h, "x-sequence-id")).collect();
            assert_eq!(seqs[0], seqs[1], "the newer text replaces the sent message");
            assert_eq!(delivery(&pool, first.id).await.0, None, "nothing left to push");
            drop_user(&pool, u).await;
        }

        #[tokio::test]
        async fn dispatch_info_leaves_no_delivery_by_default() {
            assert!(!is_recorded(kind::DISPATCH_INFO));
            assert_eq!(Push::for_kind(kind::DISPATCH_INFO, false), Push::Off);
            let Some((pool, u)) = db_user("ntfy_dispatch_info_off").await else { return };
            let n = NewNotification::new(u, kind::DISPATCH_INFO, "Dispatch received".into());
            let record = emit(&pool, on(u), n).await.unwrap();
            let deliveries: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM notification_deliveries WHERE notification_id = $1",
            )
            .bind(record.id)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(deliveries, 0);
            drop_user(&pool, u).await;
        }

        #[tokio::test]
        async fn an_approval_push_waits_for_its_escalation_delay() {
            let Some(url) = crate::routes::gateway::test_db_url("ntfy_approval_delay") else {
                return;
            };
            let pool = PgPool::connect(&url).await.expect("connect test db");
            let (u, sid) = crate::inbox::store_tests::seed_session(&pool, "ntfy-delay").await;
            sqlx::query("INSERT INTO session_viewers (session_id, pod) VALUES ($1, 'ntfy-test')")
                .bind(&sid)
                .execute(&pool)
                .await
                .unwrap();
            let (base, seen) = stub(200).await;
            let n = NewNotification {
                session_id: Some(sid.clone()),
                ..NewNotification::new(u, kind::APPROVAL, "Approve release?".into())
            };
            emit_now(&pool, on(u), &ntfy(&base), n).await;
            let (id, _, _) = notice_id(&pool, u).await;
            let (op, state, delayed, escalation) = delivery(&pool, id).await;
            assert_eq!(
                (op.as_deref(), state.as_str(), delayed, escalation),
                (Some("send"), "scheduled", true, 1)
            );
            assert!(seen.lock().unwrap().is_empty());
            sqlx::query("DELETE FROM session_viewers WHERE session_id = $1")
                .bind(&sid)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("DELETE FROM sessions WHERE id = $1")
                .bind(&sid)
                .execute(&pool)
                .await
                .unwrap();
            drop_user(&pool, u).await;
        }

        #[tokio::test]
        async fn requires_login_actions_get_an_open_button_and_no_token() {
            let Some((pool, u)) = db_user("ntfy_action_buttons").await else { return };
            let (base, seen) = stub(200).await;
            let mut config = crate::config::Config::for_test(vec![]);
            config.external_url = "https://cctui.example/".into();
            let n = NewNotification {
                actions: json!([
                    { "id": "approve", "label": "Approve" },
                    { "id": "release", "label": "Release", "requires_login": true },
                ]),
                ..NewNotification::new(u, kind::SYSTEM_ALERT, "Release?".into())
            };
            emit_now(&pool, on(u), &ntfy(&base).with_config(&config), n).await;
            let (id, _, _) = notice_id(&pool, u).await;

            let (_, _, headers) = seen.lock().unwrap().pop().unwrap();
            assert_eq!(header(&headers, "click"), "https://cctui.example/");
            let actions: Value = serde_json::from_str(header(&headers, "actions")).unwrap();
            let actions = actions.as_array().unwrap();
            assert_eq!(actions.len(), 2, "{actions:?}");
            assert_eq!(
                actions[0],
                json!({ "action": "view", "label": "Open", "url": "https://cctui.example/" })
            );
            let post = &actions[1];
            assert_eq!(
                (post["action"].as_str(), post["label"].as_str(), post["method"].as_str()),
                (Some("http"), Some("Approve"), Some("POST"))
            );
            assert_eq!(post["clear"], json!(true));
            let url = post["url"].as_str().unwrap();
            let token = url
                .strip_prefix("https://cctui.example/api/v1/notifications/actions/")
                .expect("the button posts to the token route");

            let row = tokens::peek(&pool, &tokens::hash(token).unwrap()).await.unwrap().unwrap();
            assert_eq!(
                (row.notification_id, row.action.as_str(), row.usable),
                (id, "approve", true)
            );
            assert_eq!(row.channel, "ntfy");
            let minted: Vec<(String, bool)> = sqlx::query_as(
                "SELECT action, delivery_id IS NOT NULL FROM notification_action_tokens \
                 WHERE notification_id = $1",
            )
            .bind(id)
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(minted, [("approve".to_owned(), true)], "no token for a login-only action");
            drop_user(&pool, u).await;
        }

        #[test]
        fn deep_links_point_at_the_session_or_the_app() {
            let mut config = crate::config::Config::for_test(vec![]);
            assert_eq!(dispatcher::deep_link(&config, Some("s1")), None, "no public URL");
            config.external_url = "https://cctui.example/".into();
            assert_eq!(
                dispatcher::deep_link(&config, Some("s1")).as_deref(),
                Some("https://cctui.example/sessions/s1")
            );
            assert_eq!(
                dispatcher::deep_link(&config, None).as_deref(),
                Some("https://cctui.example/")
            );
        }
    }
}
