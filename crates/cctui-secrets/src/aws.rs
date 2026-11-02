use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Context as _;
use cctui_aws::sigv4::{self, Request, Scope};
use cctui_aws::{AmbientAws, Clock, EnvLookup};

use crate::{Credential, SecretError, SecretRef};

const SERVICE: &str = "secretsmanager";
const TARGET: &str = "secretsmanager.GetSecretValue";
const CONTENT_TYPE: &str = "application/x-amz-json-1.1";

pub struct AwsSecretsManager {
    http: reqwest::Client,
    creds: AmbientAws,
    env: EnvLookup,
    now: Clock,
}

enum Fetched {
    Value(String),
    Missing,
}

impl AwsSecretsManager {
    pub fn from_process_env() -> anyhow::Result<Self> {
        Self::new(
            AmbientAws::default_chain()?,
            Arc::new(|k: &str| std::env::var(k).ok()),
            Arc::new(SystemTime::now),
        )
    }

    pub fn new(creds: AmbientAws, env: EnvLookup, now: Clock) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self { http, creds, env, now })
    }

    pub async fn get(
        &self,
        r: &SecretRef,
        name: &str,
        field: Option<&str>,
    ) -> Result<Credential, SecretError> {
        let value = match self.fetch(name).await {
            Ok(Fetched::Value(v)) => v,
            Ok(Fetched::Missing) => return Err(SecretError::not_found(r)),
            Err(e) => {
                return Err(SecretError::Backend(anyhow::anyhow!(
                    "aws secrets manager get {name}: {e:#}"
                )));
            }
        };
        let Some(field) = field else { return Ok(Credential::new(value)) };
        let json: serde_json::Value = serde_json::from_str(&value)
            .map_err(|e| SecretError::Backend(anyhow::anyhow!("aws-sm {name} is not JSON: {e}")))?;
        match json.get(field).and_then(serde_json::Value::as_str) {
            Some(v) if !v.is_empty() => Ok(Credential::new(v.to_owned())),
            _ => Err(SecretError::not_found(r)),
        }
    }

    async fn fetch(&self, name: &str) -> anyhow::Result<Fetched> {
        let region = self.region(name)?;
        let url = self.endpoint(&region)?;
        let host = host_header(&url)?;
        let body = serde_json::json!({ "SecretId": name }).to_string();
        let creds = self.creds.credentials().await.context("AWS credentials")?;
        let headers = [("content-type", CONTENT_TYPE), ("x-amz-target", TARGET)];
        let req = Request {
            method: "POST",
            host: &host,
            path: url.path(),
            query: url.query().unwrap_or_default(),
            headers: &headers,
            body: body.as_bytes(),
        };
        let scope = Scope { region: &region, service: SERVICE };
        let mut builder = self.http.post(url.clone());
        for (k, v) in headers {
            builder = builder.header(k, v);
        }
        for (k, v) in sigv4::sign(&creds, scope, &req, (self.now)()) {
            builder = builder.header(k, v);
        }
        let resp = builder.body(body).send().await.context("request")?;
        let status = resp.status();
        let error_type =
            resp.headers().get("x-amzn-errortype").and_then(|v| v.to_str().ok()).map(str::to_owned);
        let text = resp.text().await.context("reading the response")?;
        if status.is_success() {
            return parse_secret_string(&text);
        }
        let (code, message) = service_error(error_type.as_deref(), &text);
        if code == "ResourceNotFoundException" {
            return Ok(Fetched::Missing);
        }
        anyhow::bail!("{code} ({status}): {message}")
    }

    fn var(&self, name: &str) -> Option<String> {
        (self.env)(name).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
    }

    fn region(&self, name: &str) -> anyhow::Result<String> {
        arn_region(name)
            .map(str::to_owned)
            .or_else(|| self.var("AWS_REGION"))
            .or_else(|| self.var("AWS_DEFAULT_REGION"))
            .ok_or_else(|| anyhow::anyhow!("no AWS region: set AWS_REGION or use the secret ARN"))
    }

    fn endpoint(&self, region: &str) -> anyhow::Result<reqwest::Url> {
        let url = self
            .var("AWS_ENDPOINT_URL_SECRETS_MANAGER")
            .or_else(|| self.var("AWS_ENDPOINT_URL"))
            .unwrap_or_else(|| default_endpoint(region));
        reqwest::Url::parse(&url).with_context(|| format!("secrets manager endpoint {url:?}"))
    }
}

fn arn_region(name: &str) -> Option<&str> {
    let mut parts = name.strip_prefix("arn:")?.split(':');
    let (_partition, service, region) = (parts.next()?, parts.next()?, parts.next()?);
    (service == SERVICE && !region.is_empty()).then_some(region)
}

fn default_endpoint(region: &str) -> String {
    let suffix = if region.starts_with("cn-") { "amazonaws.com.cn" } else { "amazonaws.com" };
    format!("https://{SERVICE}.{region}.{suffix}/")
}

fn host_header(url: &reqwest::Url) -> anyhow::Result<String> {
    let host = url.host_str().ok_or_else(|| anyhow::anyhow!("endpoint {url} has no host"))?;
    Ok(url.port().map_or_else(|| host.to_owned(), |port| format!("{host}:{port}")))
}

fn parse_secret_string(body: &str) -> anyhow::Result<Fetched> {
    let json: serde_json::Value = serde_json::from_str(body).map_err(|e| {
        anyhow::anyhow!("response is not JSON (line {}, column {})", e.line(), e.column())
    })?;
    Ok(json
        .get("SecretString")
        .and_then(serde_json::Value::as_str)
        .filter(|v| !v.is_empty())
        .map_or(Fetched::Missing, |v| Fetched::Value(v.to_owned())))
}

fn service_error(header: Option<&str>, body: &str) -> (String, String) {
    let json: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    let code = header.or_else(|| json.get("__type").and_then(serde_json::Value::as_str)).map_or(
        "UnknownError",
        |t| {
            let t = t.split(':').next().unwrap_or(t);
            t.rsplit('#').next().unwrap_or(t)
        },
    );
    let message = json
        .get("message")
        .or_else(|| json.get("Message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    (code.to_owned(), message.to_owned())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use cctui_aws::{AwsCredentials, FixedSource};

    use super::*;
    use crate::{Engines, RefResolver as _, resolve_value};

    const NOW_SECS: u64 = 1_767_323_045;
    const NOW_AMZ: &str = "20260102T030405Z";

    fn clock() -> Clock {
        Arc::new(|| SystemTime::UNIX_EPOCH + Duration::from_secs(NOW_SECS))
    }

    fn creds() -> AwsCredentials {
        let mut c = AwsCredentials::new("ASIATEST", "secret-key");
        c.session_token = Some("session-token".to_owned());
        c
    }

    fn header<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
        headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default()
    }

    fn json_error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
        let body = serde_json::json!({ "__type": code, "Message": message }).to_string();
        (status, [("content-type", CONTENT_TYPE)], body).into_response()
    }

    fn check_request(headers: &HeaderMap, body: &str) -> Result<(), String> {
        let expect = [
            ("content-type", CONTENT_TYPE),
            ("x-amz-target", TARGET),
            ("x-amz-date", NOW_AMZ),
            ("x-amz-security-token", "session-token"),
        ];
        for (k, v) in expect {
            if header(headers, k) != v {
                return Err(format!("header {k}: {:?}", header(headers, k)));
            }
        }
        let signed_headers = [("content-type", CONTENT_TYPE), ("x-amz-target", TARGET)];
        let req = Request {
            method: "POST",
            host: header(headers, "host"),
            path: "/",
            query: "",
            headers: &signed_headers,
            body: body.as_bytes(),
        };
        let scope = Scope { region: "us-east-1", service: SERVICE };
        let signed = sigv4::sign(&creds(), scope, &req, clock()());
        let want = &signed.iter().find(|(k, _)| *k == "authorization").unwrap().1;
        let got = header(headers, "authorization");
        if got != want {
            return Err(format!("authorization {got:?} != {want:?}"));
        }
        if !got.contains(
            "Credential=ASIATEST/20260102/us-east-1/secretsmanager/aws4_request, \
             SignedHeaders=content-type;host;x-amz-date;x-amz-security-token;x-amz-target, ",
        ) {
            return Err(format!("authorization scope {got:?}"));
        }
        Ok(())
    }

    async fn spawn_sm_mock() -> String {
        use axum::routing::post;

        let app = axum::Router::new().route(
            "/",
            post(|headers: HeaderMap, body: String| async move {
                if let Err(why) = check_request(&headers, &body) {
                    return json_error(StatusCode::BAD_REQUEST, "InvalidSignatureException", &why);
                }
                let req: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                let secret = match req["SecretId"].as_str() {
                    Some("cctui/plain") => "sm-plain",
                    Some("cctui/json") => r#"{"password":"sm-pw","user":"u"}"#,
                    Some("cctui/denied") => {
                        return json_error(
                            StatusCode::BAD_REQUEST,
                            "AccessDeniedException",
                            "not authorized to perform secretsmanager:GetSecretValue",
                        );
                    }
                    Some("cctui/badsig") => {
                        return (
                            StatusCode::BAD_REQUEST,
                            [(
                                "x-amzn-errortype",
                                "InvalidSignatureException:http://internal.amazon.com/coral/",
                            )],
                            r#"{"message":"The request signature we calculated does not match"}"#,
                        )
                            .into_response();
                    }
                    Some("cctui/namespaced") => {
                        return json_error(
                            StatusCode::BAD_REQUEST,
                            "com.amazonaws.secretsmanager#ResourceNotFoundException",
                            "no such secret",
                        );
                    }
                    _ => {
                        return json_error(
                            StatusCode::BAD_REQUEST,
                            "ResourceNotFoundException",
                            "no such secret",
                        );
                    }
                };
                let out = serde_json::json!({"Name": req["SecretId"], "SecretString": secret});
                ([("content-type", CONTENT_TYPE)], out.to_string()).into_response()
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn manager(vars: &[(&str, &str)]) -> AwsSecretsManager {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let vars: HashMap<String, String> =
            vars.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        let ambient = AmbientAws::new(Box::new(FixedSource(creds())), clock());
        AwsSecretsManager::new(ambient, Arc::new(move |k: &str| vars.get(k).cloned()), clock())
            .unwrap()
    }

    fn sm_engines(base: &str) -> Engines {
        Engines::new().with_aws(manager(&[("AWS_REGION", "us-east-1"), ("AWS_ENDPOINT_URL", base)]))
    }

    async fn backend_error(engines: &Engines, r: &str) -> String {
        let r = SecretRef::parse(r).unwrap();
        match engines.resolve(&r).await {
            Err(SecretError::Backend(e)) => format!("{e:#}"),
            other => panic!("{r} must be a backend error, got {:?}", other.map(|_| "value")),
        }
    }

    #[tokio::test]
    async fn aws_sm_engine_reads_plain_and_json_field() {
        let engines = sm_engines(&spawn_sm_mock().await);
        let plain = SecretRef::parse("aws-sm:cctui/plain").unwrap();
        assert_eq!(engines.resolve(&plain).await.unwrap().expose(), "sm-plain");
        let field = SecretRef::parse("aws-sm:cctui/json#password").unwrap();
        assert_eq!(engines.resolve(&field).await.unwrap().expose(), "sm-pw");
    }

    #[tokio::test]
    async fn aws_sm_engine_missing_secret_or_field_is_not_found() {
        let engines = sm_engines(&spawn_sm_mock().await);
        for r in ["aws-sm:cctui/nope", "aws-sm:cctui/namespaced", "aws-sm:cctui/json#missing"] {
            let r = SecretRef::parse(r).unwrap();
            assert!(
                matches!(engines.resolve(&r).await, Err(SecretError::NotFound { .. })),
                "{r} must be NotFound"
            );
        }
    }

    #[tokio::test]
    async fn aws_sm_access_denied_and_signature_errors_are_backend_failures() {
        let engines = sm_engines(&spawn_sm_mock().await);
        let denied = backend_error(&engines, "aws-sm:cctui/denied").await;
        assert!(denied.contains("cctui/denied"), "{denied}");
        assert!(denied.contains("AccessDeniedException (400 Bad Request)"), "{denied}");
        assert!(denied.contains("not authorized"), "{denied}");
        let badsig = backend_error(&engines, "aws-sm:cctui/badsig").await;
        assert!(badsig.contains("InvalidSignatureException (400 Bad Request)"), "{badsig}");
        assert!(!badsig.contains("internal.amazon.com"), "{badsig}");
    }

    #[tokio::test]
    async fn aws_sm_request_is_signed_for_the_received_host_and_body() {
        let engines = sm_engines(&spawn_sm_mock().await);
        let r = SecretRef::parse("aws-sm:cctui/plain").unwrap();
        assert!(engines.resolve(&r).await.is_ok(), "the mock rejects a bad signature");

        let mut wrong = creds();
        wrong.secret_key = "other-key".to_owned();
        let ambient = AmbientAws::new(Box::new(FixedSource(wrong)), clock());
        let vars: HashMap<String, String> = [
            ("AWS_REGION".to_owned(), "us-east-1".to_owned()),
            ("AWS_ENDPOINT_URL".to_owned(), spawn_sm_mock().await),
        ]
        .into();
        let sm =
            AwsSecretsManager::new(ambient, Arc::new(move |k: &str| vars.get(k).cloned()), clock())
                .unwrap();
        let msg = backend_error(&Engines::new().with_aws(sm), "aws-sm:cctui/plain").await;
        assert!(msg.contains("InvalidSignatureException"), "{msg}");
    }

    #[tokio::test]
    async fn aws_sm_engine_field_on_non_json_fails_closed() {
        let engines = sm_engines(&spawn_sm_mock().await);
        let r = SecretRef::parse("aws-sm:cctui/plain#password").unwrap();
        assert!(matches!(engines.resolve(&r).await, Err(SecretError::Backend(_))));
    }

    #[tokio::test]
    async fn aws_sm_value_error_names_the_ref_and_hides_the_secret() {
        let engines = sm_engines(&spawn_sm_mock().await);
        let msg =
            resolve_value(&engines, "aws-sm:cctui/json#missing").await.unwrap_err().to_string();
        assert!(msg.contains("aws-sm:cctui/json#missing"), "{msg}");
        assert!(!msg.contains("sm-pw"), "{msg}");
    }

    #[test]
    fn region_comes_from_the_arn_then_the_environment() {
        let arn = "arn:aws:secretsmanager:eu-west-3:123456789012:secret:cctui/x-AbCdEf";
        let sm = manager(&[("AWS_REGION", "us-east-1"), ("AWS_DEFAULT_REGION", "ap-south-1")]);
        assert_eq!(sm.region(arn).unwrap(), "eu-west-3");
        assert_eq!(sm.region("cctui/x").unwrap(), "us-east-1");
        let sm = manager(&[("AWS_DEFAULT_REGION", "ap-south-1")]);
        assert_eq!(sm.region("cctui/x").unwrap(), "ap-south-1");
        assert!(manager(&[]).region("cctui/x").is_err());
        assert_eq!(arn_region("arn:aws:s3:eu-west-3:1:secret:x"), None);
    }

    #[test]
    fn endpoint_defaults_to_the_regional_service_and_honours_overrides() {
        let sm = manager(&[]);
        assert_eq!(
            sm.endpoint("eu-west-3").unwrap().as_str(),
            "https://secretsmanager.eu-west-3.amazonaws.com/"
        );
        assert_eq!(
            sm.endpoint("cn-north-1").unwrap().as_str(),
            "https://secretsmanager.cn-north-1.amazonaws.com.cn/"
        );
        let sm = manager(&[
            ("AWS_ENDPOINT_URL", "http://127.0.0.1:1/"),
            ("AWS_ENDPOINT_URL_SECRETS_MANAGER", "http://127.0.0.1:4566"),
        ]);
        let url = sm.endpoint("eu-west-3").unwrap();
        assert_eq!(host_header(&url).unwrap(), "127.0.0.1:4566");
        let regional = manager(&[]).endpoint("eu-west-3").unwrap();
        assert_eq!(host_header(&regional).unwrap(), "secretsmanager.eu-west-3.amazonaws.com");
    }

    #[test]
    fn success_body_without_a_secret_string_is_missing_and_never_echoed() {
        assert!(matches!(
            parse_secret_string(r#"{"SecretBinary":"AAEC"}"#).unwrap(),
            Fetched::Missing
        ));
        let err = parse_secret_string("leak-me").map(|_| ()).unwrap_err().to_string();
        assert!(!err.contains("leak-me"), "{err}");
    }
}
