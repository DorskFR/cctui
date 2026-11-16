use std::sync::OnceLock;

use serde_json::{Value, json};

use super::channel::{Capabilities, Channel, Endpoint, ProviderRef, Rendered, SendError};
use crate::config::Config;

pub const ID: &str = "ntfy";

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(crate::build_http_client)
}

pub struct NtfyChannel {
    topic_url: String,
    token: cctui_secrets::Secret,
}

impl NtfyChannel {
    #[must_use]
    pub fn from_config(config: &Config) -> Option<Self> {
        Some(Self::new(config.ntfy_url.as_deref()?, config.ntfy_token.clone()?))
    }

    #[must_use]
    pub fn new(topic_url: &str, token: cctui_secrets::Secret) -> Self {
        Self { topic_url: topic_url.trim_end_matches('/').to_owned(), token }
    }

    /// The server token only ever goes to the server's own topic.
    fn topic(&self, to: &Endpoint) -> Result<&str, SendError> {
        match to.config.get("source").and_then(Value::as_str) {
            Some("env") => Ok(&self.topic_url),
            _ => Err(SendError::Permanent("unsupported ntfy endpoint".into())),
        }
    }

    async fn publish(&self, n: &Rendered, topic: &str, sequence_id: &str) -> Result<(), SendError> {
        let mut req = client()
            .post(topic)
            .bearer_auth(self.token.get())
            .header("X-Sequence-ID", sequence_id)
            .header("Title", &n.title)
            .header("Priority", n.priority.to_string());
        if let Some(tags) = &n.tags {
            req = req.header("Tags", tags);
        }
        classify(req.body(n.body.clone().unwrap_or_default()).send().await).await
    }
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
        self.publish(n, self.topic(to)?, &sequence_id).await?;
        Ok(ProviderRef(json!({ "sequence_id": sequence_id })))
    }

    async fn update(&self, n: &Rendered, to: &Endpoint, r: &ProviderRef) -> Result<(), SendError> {
        self.publish(n, self.topic(to)?, sequence_id(r)?).await
    }

    async fn retract(&self, to: &Endpoint, r: &ProviderRef) -> Result<(), SendError> {
        let url = format!("{}/{}/clear", self.topic(to)?, sequence_id(r)?);
        classify(client().put(url).bearer_auth(self.token.get()).send().await).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::json;

    use super::NtfyChannel;
    use crate::notify::channel::{Channel, Endpoint, ProviderRef, Rendered, SendError};

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
}
