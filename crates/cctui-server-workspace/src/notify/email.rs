use std::fmt::Write as _;
use std::time::Duration;

use lettre::message::{Mailbox, MultiPart};
use lettre::{Address, AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde_json::{Value, json};

use super::channel::{Capabilities, Channel, Endpoint, ProviderRef, Rendered, SendError};
use super::store::clip;
use crate::config::Config;

pub const ID: &str = "email";

const SMTP_TIMEOUT: Duration = Duration::from_secs(8);

enum Mailer {
    Smtp(cctui_secrets::Secret),
    #[cfg(test)]
    Stub(lettre::transport::stub::AsyncStubTransport),
}

pub struct EmailChannel {
    from: Mailbox,
    mailer: Mailer,
}

pub fn address(config: &Value) -> Result<Address, String> {
    let raw = config
        .get("address")
        .and_then(Value::as_str)
        .ok_or("email endpoint has no address")?;
    raw.trim().parse::<Address>().map_err(|e| format!("invalid email address: {e}"))
}

struct Mail {
    subject: String,
    text: String,
    html: String,
}

fn one_line(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn links(n: &Rendered) -> Vec<(&str, &str)> {
    let open = n.link.as_deref().map(|url| ("Open in cctui", url));
    let actions = n.buttons.iter().map(|b| (b.label.as_str(), b.url.as_str()));
    open.into_iter().chain(actions).collect()
}

fn render(n: &Rendered) -> Mail {
    let subject = one_line(&n.title);
    let body = n.body.as_deref().map(clip);
    let links = links(n);
    let mut text = format!("{subject}\n");
    let mut html = format!("<!DOCTYPE html>\n<html><body>\n<p><b>{}</b></p>\n", escape(&subject));
    if let Some(body) = &body {
        let _ = write!(text, "\n{body}\n");
        let _ = writeln!(html, "<p>{}</p>", escape(body));
    }
    if !links.is_empty() {
        text.push('\n');
        html.push_str("<ul>\n");
    }
    for (label, url) in &links {
        let _ = writeln!(text, "{label}: {url}");
        let _ = writeln!(html, "<li><a href=\"{}\">{}</a></li>", escape(url), escape(label));
    }
    if !links.is_empty() {
        html.push_str("</ul>\n");
    }
    html.push_str("</body></html>\n");
    Mail { subject, text, html }
}

fn classify(e: &lettre::transport::smtp::Error) -> SendError {
    verdict(e.is_permanent() || e.is_client(), e.to_string())
}

fn verdict(permanent: bool, detail: String) -> SendError {
    if permanent { SendError::Permanent(detail) } else { SendError::Retry(detail) }
}

impl EmailChannel {
    #[must_use]
    pub fn from_config(config: &Config) -> Option<Self> {
        let url = config.smtp_url.clone()?;
        let from = config.smtp_from.as_deref()?;
        match from.trim().parse::<Mailbox>() {
            Ok(from) => Some(Self { from, mailer: Mailer::Smtp(url) }),
            Err(e) => {
                tracing::warn!("email channel off: CCTUI_SMTP_FROM is invalid: {e}");
                None
            }
        }
    }

    fn message_id(&self, n: &Rendered) -> String {
        format!("<{}@{}>", n.delivery_id, self.from.email.domain())
    }

    fn message(&self, n: &Rendered, to: Address) -> Result<Message, SendError> {
        let mail = render(n);
        Message::builder()
            .from(self.from.clone())
            .to(Mailbox::new(None, to))
            .subject(mail.subject)
            .message_id(Some(self.message_id(n)))
            .multipart(MultiPart::alternative_plain_html(mail.text, mail.html))
            .map_err(|e| SendError::Permanent(e.to_string()))
    }

    async fn deliver(&self, message: Message) -> Result<(), SendError> {
        match &self.mailer {
            Mailer::Smtp(url) => {
                let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(&url.get())
                    .map_err(|_| SendError::Permanent("CCTUI_SMTP_URL is invalid".into()))?
                    .timeout(Some(SMTP_TIMEOUT))
                    .build();
                transport.send(message).await.map(|_| ()).map_err(|e| classify(&e))
            }
            #[cfg(test)]
            Mailer::Stub(stub) => {
                stub.send(message).await.map_err(|e| SendError::Retry(e.to_string()))
            }
        }
    }
}

#[async_trait::async_trait]
impl Channel for EmailChannel {
    fn id(&self) -> &'static str {
        ID
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { update: false, retract: false, buttons: 0, needs_token: true }
    }

    async fn send(&self, n: &Rendered, to: &Endpoint) -> Result<ProviderRef, SendError> {
        let rcpt = address(&to.config).map_err(SendError::Permanent)?;
        let message = self.message(n, rcpt)?;
        self.deliver(message).await?;
        Ok(ProviderRef(json!({ "message_id": self.message_id(n) })))
    }

    async fn update(&self, _: &Rendered, _: &Endpoint, _: &ProviderRef) -> Result<(), SendError> {
        Ok(())
    }

    async fn retract(&self, _: &Endpoint, _: &ProviderRef) -> Result<(), SendError> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "email_tests.rs"]
mod tests;
