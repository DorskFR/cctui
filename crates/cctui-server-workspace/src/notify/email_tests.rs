use std::sync::Arc;

use lettre::transport::stub::AsyncStubTransport;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use uuid::Uuid;

use super::{
    EmailChannel, ID, Mailer, SEND_BUDGET, SMTP_COMMAND_TIMEOUT, address, render, verdict,
};
use crate::config::Config;
use crate::notify::channel::{
    Button, Capabilities, Channel, Channels, Endpoint, Press, ProviderRef, Rendered, SendError,
};
use crate::notify::store::BODY_MAX_CHARS;

const TOKEN: &str = "q2b9ZbXk1x7m0Yw3pR8sT4uV6wX9yZ0aB1cD2eF3gH4";

fn confirm(token: &str) -> String {
    format!("https://cctui.example/n/{token}")
}

fn rendered() -> Rendered {
    Rendered {
        delivery_id: Uuid::from_u128(7),
        title: "Approve release?".into(),
        body: Some("v1.2 is waiting on you".into()),
        priority: 4,
        tags: None,
        link: Some("https://cctui.example/sessions/s1".into()),
        buttons: vec![Button { label: "Approve".into(), url: confirm(TOKEN), press: Press::Open }],
    }
}

fn stub_channel(stub: &AsyncStubTransport) -> EmailChannel {
    EmailChannel {
        from: "cctui <noreply@cctui.example>".parse().unwrap(),
        mailer: Mailer::Stub(stub.clone()),
    }
}

fn smtp_channel(url: &str) -> EmailChannel {
    EmailChannel {
        from: "noreply@cctui.example".parse().unwrap(),
        mailer: Mailer::Smtp(cctui_secrets::Secret::fixed("CCTUI_SMTP_URL", url)),
    }
}

fn to(address: &str) -> Endpoint {
    Endpoint { config: json!({ "address": address }) }
}

#[test]
fn the_mail_carries_the_title_body_and_confirm_links() {
    let mail = render(&rendered());
    assert_eq!(mail.subject, "Approve release?");
    assert_eq!(
        mail.text,
        "Approve release?\n\nv1.2 is waiting on you\n\n\
         Open in cctui: https://cctui.example/sessions/s1\n\
         Approve: https://cctui.example/n/q2b9ZbXk1x7m0Yw3pR8sT4uV6wX9yZ0aB1cD2eF3gH4\n"
    );
    assert!(mail.html.contains("<p><b>Approve release?</b></p>"), "{}", mail.html);
    assert!(mail.html.contains("<p>v1.2 is waiting on you</p>"), "{}", mail.html);
    assert!(mail.html.contains(&format!("<li><a href=\"{}\">Approve</a></li>", confirm(TOKEN))));
    assert!(
        mail.html.contains("<li><a href=\"https://cctui.example/sessions/s1\">Open in cctui</a>")
    );
    assert!(!mail.html.contains("<img"), "no images, no tracking pixels");
}

#[test]
fn the_token_only_ever_appears_inside_its_confirm_link() {
    let mail = render(&rendered());
    for part in [&mail.text, &mail.html] {
        assert_eq!(part.matches(TOKEN).count(), 1, "{part}");
        assert_eq!(part.matches(&confirm(TOKEN)).count(), 1, "{part}");
    }
    assert!(!mail.subject.contains(TOKEN));
}

#[test]
fn the_body_is_clipped_to_one_short_line() {
    let long = "x".repeat(BODY_MAX_CHARS + 100);
    let mail = render(&Rendered { body: Some(long.clone()), ..rendered() });
    assert!(!mail.text.contains(&long));
    let line = mail.text.lines().nth(2).unwrap();
    assert_eq!(line.chars().count(), BODY_MAX_CHARS, "{line}");
    assert!(line.ends_with('…'));
}

#[test]
fn a_bare_notice_has_no_link_list() {
    let mail = render(&Rendered { body: None, link: None, buttons: Vec::new(), ..rendered() });
    assert_eq!(mail.text, "Approve release?\n");
    assert!(!mail.html.contains("<ul>"));
}

#[test]
fn html_is_escaped_and_the_subject_stays_on_one_line() {
    let mail = render(&Rendered {
        title: "a\r\nBcc: evil@x".into(),
        body: Some("<script>\"&'</script>".into()),
        ..rendered()
    });
    assert_eq!(mail.subject, "a  Bcc: evil@x");
    assert!(mail.html.contains("<p>&lt;script&gt;&quot;&amp;&#39;&lt;/script&gt;</p>"));
    assert!(!mail.html.contains("<script>"));
}

#[test]
fn smtp_settings_unset_register_no_email_channel() {
    let mut config = Config::for_test(vec![]);
    assert!(Channels::from_config(&config).get(ID).is_none());
    config.smtp_url = Some(cctui_secrets::Secret::fixed("CCTUI_SMTP_URL", "smtp://localhost"));
    assert!(Channels::from_config(&config).get(ID).is_none(), "no sender, no channel");
    config.smtp_from = Some("not a mailbox".into());
    assert!(Channels::from_config(&config).get(ID).is_none(), "an invalid sender is off");
    config.smtp_from = Some("cctui <noreply@cctui.example>".into());
    assert!(Channels::from_config(&config).get(ID).is_some());
    config.smtp_url = None;
    assert!(Channels::from_config(&config).get(ID).is_none(), "no relay, no channel");
}

#[test]
fn endpoint_addresses_go_through_the_lettre_parser() {
    let parsed = address(&json!({ "address": " me@example.com " })).unwrap();
    assert_eq!(parsed.to_string(), "me@example.com");
    assert!(address(&json!({})).is_err());
    assert!(address(&json!({ "address": 3 })).is_err());
    assert!(address(&json!({ "address": "not an address" })).is_err());
    assert!(address(&json!({ "address": "me@example.com\r\nBcc: x@y" })).is_err());
}

#[tokio::test]
async fn email_is_send_only() {
    let ch = stub_channel(&AsyncStubTransport::new_ok());
    assert_eq!(
        ch.capabilities(),
        Capabilities { update: false, retract: false, buttons: 0, needs_token: true }
    );
    assert!(SMTP_COMMAND_TIMEOUT < SEND_BUDGET && SEND_BUDGET < ch.call_timeout());
    assert!(ch.call_timeout() > crate::notify::channel::DEFAULT_CALL_TIMEOUT);
    let r = ProviderRef(json!({ "message_id": "<x@y>" }));
    ch.update(&rendered(), &to("me@example.com"), &r).await.unwrap();
    ch.retract(&to("me@example.com"), &r).await.unwrap();
}

#[tokio::test]
async fn a_send_produces_one_message_to_the_endpoint_address() {
    let stub = AsyncStubTransport::new_ok();
    let r = stub_channel(&stub).send(&rendered(), &to("me@example.com")).await.unwrap();
    let message_id = format!("<{}@cctui.example>", Uuid::from_u128(7));
    assert_eq!(r, ProviderRef(json!({ "message_id": message_id })));

    let sent = stub.messages().await;
    assert_eq!(sent.len(), 1);
    let (envelope, raw) = &sent[0];
    let recipients: Vec<String> = envelope.to().iter().map(ToString::to_string).collect();
    assert_eq!(recipients, ["me@example.com"]);
    assert_eq!(envelope.from().map(ToString::to_string).as_deref(), Some("noreply@cctui.example"));
    assert!(raw.contains("Subject: Approve release?\r\n"), "{raw}");
    assert!(raw.contains(&format!("Message-ID: {message_id}\r\n")), "{raw}");
    assert!(raw.contains("multipart/alternative"), "{raw}");
    assert!(raw.contains("text/plain"), "{raw}");
    assert!(raw.contains("text/html"), "{raw}");
    assert!(raw.contains(&format!("Approve: {}", confirm(TOKEN))), "{raw}");
}

#[tokio::test]
async fn a_bad_endpoint_address_is_permanent_and_sends_nothing() {
    let stub = AsyncStubTransport::new_ok();
    let err = stub_channel(&stub).send(&rendered(), &to("nope")).await.unwrap_err();
    assert!(matches!(err, SendError::Permanent(_)), "{err:?}");
    assert!(stub.messages().await.is_empty());
}

#[test]
fn refusals_are_permanent_and_everything_else_retries() {
    assert_eq!(verdict(true, "550".into()), SendError::Permanent("550".into()));
    assert_eq!(verdict(false, "450".into()), SendError::Retry("450".into()));
}

async fn fake_smtp(rcpt: &'static str) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        let (read, mut write) = sock.into_split();
        let mut lines = BufReader::new(read).lines();
        write.write_all(b"220 fake ESMTP\r\n").await.unwrap();
        let mut in_data = false;
        while let Ok(Some(line)) = lines.next_line().await {
            let reply = if in_data {
                if line != "." {
                    continue;
                }
                in_data = false;
                "250 queued\r\n"
            } else {
                match line.get(..4).unwrap_or_default().to_ascii_uppercase().as_str() {
                    "RCPT" => rcpt,
                    "DATA" => {
                        in_data = true;
                        "354 go ahead\r\n"
                    }
                    "QUIT" => "221 bye\r\n",
                    _ => "250 ok\r\n",
                }
            };
            if write.write_all(reply.as_bytes()).await.is_err() {
                break;
            }
        }
    });
    port
}

#[tokio::test]
async fn smtp_replies_map_to_retry_or_permanent() {
    for (rcpt, outcome) in [
        ("250 ok\r\n", "sent"),
        ("450 4.2.1 mailbox busy\r\n", "retry"),
        ("421 4.3.2 shutting down\r\n", "retry"),
        ("550 5.1.1 no such user\r\n", "permanent"),
        ("553 5.1.3 bad address\r\n", "permanent"),
    ] {
        let port = fake_smtp(rcpt).await;
        let ch = smtp_channel(&format!("smtp://127.0.0.1:{port}"));
        let got = match ch.send(&rendered(), &to("me@example.com")).await {
            Ok(_) => "sent",
            Err(SendError::Retry(_)) => "retry",
            Err(SendError::Permanent(_)) => "permanent",
            Err(SendError::Gone) => "gone",
        };
        assert_eq!(got, outcome, "{rcpt}");
    }
}

#[tokio::test]
async fn an_unreachable_relay_retries() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let ch = smtp_channel(&format!("smtp://127.0.0.1:{port}"));
    let err = ch.send(&rendered(), &to("me@example.com")).await.unwrap_err();
    assert!(matches!(err, SendError::Retry(_)), "{err:?}");
}

#[tokio::test]
async fn a_bad_relay_url_is_permanent_and_never_echoes_the_password() {
    let ch = smtp_channel("ftp://user:hunter2@relay.example");
    let err = ch.send(&rendered(), &to("me@example.com")).await.unwrap_err();
    assert_eq!(err, SendError::Permanent("CCTUI_SMTP_URL is invalid".into()));
}

#[tokio::test]
async fn the_dispatcher_mails_confirm_links_bound_to_email_tokens() {
    use crate::notify::store::{NewNotification, Routing, emit};
    use crate::notify::{dispatcher, kind, tokens};

    let Some(url) = crate::routes::gateway::test_db_url("email_dispatch_confirm_links") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
    let u = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(u)
        .bind(format!("email-{u}"))
        .bind(format!("email-key-{u}"))
        .execute(&pool)
        .await
        .unwrap();
    let n = NewNotification {
        actions: json!([
            { "id": "approve", "label": "Approve" },
            { "id": "release", "label": "Release", "requires_login": true },
        ]),
        ..NewNotification::new(u, kind::SYSTEM_ALERT, "Release?".into())
    };
    let notice = emit(&pool, Routing::default(), n).await.unwrap();
    let endpoint: Uuid = sqlx::query_scalar(
        "INSERT INTO notification_endpoints (user_id, channel, config) \
         VALUES ($1, 'email', '{\"address\": \"me@example.com\"}') RETURNING id",
    )
    .bind(u)
    .fetch_one(&pool)
    .await
    .unwrap();
    let delivery: Uuid = sqlx::query_scalar(
        "INSERT INTO notification_deliveries (notification_id, channel, endpoint_id, op) \
         VALUES ($1, 'email', $2, 'send') RETURNING id",
    )
    .bind(notice.id)
    .bind(endpoint)
    .fetch_one(&pool)
    .await
    .unwrap();

    let stub = AsyncStubTransport::new_ok();
    let mut config = Config::for_test(vec![]);
    config.external_url = "https://cctui.example/".into();
    let mut channels = Channels::default();
    channels.add(Arc::new(stub_channel(&stub)));
    dispatcher::run(&pool, &channels.with_config(&config), Some(notice.id)).await;

    let sent = stub.messages().await;
    assert_eq!(sent.len(), 1);
    let (envelope, raw) = &sent[0];
    let recipients: Vec<String> = envelope.to().iter().map(ToString::to_string).collect();
    assert_eq!(recipients, ["me@example.com"]);
    assert!(raw.contains("Open in cctui: https://cctui.example/\r\n"), "{raw}");
    assert!(!raw.contains("/api/v1/notifications/actions/"), "no POST links in mail");
    assert!(!raw.contains("Release:"), "login-only actions get no link");
    let prefix = "Approve: https://cctui.example/n/";
    let at = raw.find(prefix).expect("a confirm link") + prefix.len();
    let token = &raw[at..at + 43];

    let row = tokens::peek(&pool, &tokens::hash(token).unwrap()).await.unwrap().unwrap();
    assert_eq!(
        (row.notification_id, row.action.as_str(), row.channel.as_str(), row.usable),
        (notice.id, "approve", "email", true)
    );
    let (state, bound): (String, i64) = sqlx::query_as(
        "SELECT d.state, (SELECT count(*) FROM notification_action_tokens t \
                          WHERE t.delivery_id = d.id) \
         FROM notification_deliveries d WHERE d.id = $1",
    )
    .bind(delivery)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((state.as_str(), bound), ("sent", 1));

    sqlx::query("DELETE FROM users WHERE id = $1").bind(u).execute(&pool).await.unwrap();
}
