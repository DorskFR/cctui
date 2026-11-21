use std::fmt;
use std::time::{Duration, SystemTime};

use reqwest::{Method, StatusCode};
use sha2::{Digest as _, Sha256};

use crate::AwsCredentials;
use crate::sigv4::{self, Request, Scope, UNSIGNED_PAYLOAD};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_TIMEOUT: Duration = Duration::from_mins(1);
const MAX_KEY_BYTES: usize = 1024;

pub struct S3Client {
    http: reqwest::Client,
    origin: String,
    host: String,
    bucket: String,
    region: String,
    creds: AwsCredentials,
}

impl fmt::Debug for S3Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3Client")
            .field("origin", &self.origin)
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("creds", &self.creds)
            .finish_non_exhaustive()
    }
}

impl S3Client {
    pub fn new(
        endpoint: &str,
        bucket: &str,
        region: &str,
        creds: AwsCredentials,
    ) -> anyhow::Result<Self> {
        Self::with_timeout(endpoint, bucket, region, creds, DEFAULT_TIMEOUT)
    }

    pub fn with_timeout(
        endpoint: &str,
        bucket: &str,
        region: &str,
        creds: AwsCredentials,
        timeout: Duration,
    ) -> anyhow::Result<Self> {
        let (origin, host) = parse_endpoint(endpoint)?;
        anyhow::ensure!(
            !bucket.is_empty() && !bucket.contains('/') && bucket != "." && bucket != "..",
            "invalid S3 bucket name {bucket:?}"
        );
        anyhow::ensure!(!region.is_empty(), "S3 region is empty");
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self { http, origin, host, bucket: bucket.to_owned(), region: region.to_owned(), creds })
    }

    pub async fn put_object(
        &self,
        key: &str,
        bytes: &[u8],
        sha256_hex: &str,
    ) -> anyhow::Result<()> {
        let resp = self.send(Method::PUT, key, Some((bytes, sha256_hex))).await?;
        if resp.status().is_success() {
            return Ok(());
        }
        Err(failure(resp, "PUT", key).await)
    }

    pub async fn get_object(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let resp = self.send(Method::GET, key, None).await?;
        match resp.status() {
            StatusCode::NOT_FOUND => Ok(None),
            s if s.is_success() => {
                let body = resp.bytes().await.map_err(|e| transport("GET", key, e))?;
                Ok(Some(body.to_vec()))
            }
            _ => Err(failure(resp, "GET", key).await),
        }
    }

    pub async fn head_object(&self, key: &str) -> anyhow::Result<Option<u64>> {
        let resp = self.send(Method::HEAD, key, None).await?;
        match resp.status() {
            StatusCode::NOT_FOUND => Ok(None),
            s if s.is_success() => resp
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(Some)
                .ok_or_else(|| anyhow::anyhow!("s3 HEAD {key}: no content-length")),
            s => anyhow::bail!("s3 HEAD {key}: {s}"),
        }
    }

    pub async fn delete_object(&self, key: &str) -> anyhow::Result<()> {
        let resp = self.send(Method::DELETE, key, None).await?;
        let status = resp.status();
        if status.is_success() || status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        Err(failure(resp, "DELETE", key).await)
    }

    async fn send(
        &self,
        method: Method,
        key: &str,
        body: Option<(&[u8], &str)>,
    ) -> anyhow::Result<reqwest::Response> {
        check_key(key)?;
        let path = sigv4::s3_canonical_uri(&format!("/{}/{key}", self.bucket));
        let payload_hash = body.map_or(UNSIGNED_PAYLOAD, |(_, hash)| hash);
        let headers = [("x-amz-content-sha256", payload_hash)];
        let req = Request {
            method: method.as_str(),
            host: &self.host,
            path: &path,
            query: "",
            headers: &headers,
            body: b"",
        };
        let scope = Scope { region: &self.region, service: "s3" };
        let now = SystemTime::now();
        let signed = sigv4::sign_with_payload_hash(&self.creds, scope, &req, payload_hash, now);
        let op = method.as_str().to_owned();
        let mut builder = self
            .http
            .request(method, format!("{}{path}", self.origin))
            .header("x-amz-content-sha256", payload_hash);
        for (k, v) in signed {
            builder = builder.header(k, v);
        }
        if let Some((bytes, _)) = body {
            builder = builder.body(bytes.to_vec());
        }
        builder.send().await.map_err(|e| transport(&op, key, e))
    }
}

fn parse_endpoint(endpoint: &str) -> anyhow::Result<(String, String)> {
    let url = reqwest::Url::parse(endpoint.trim())
        .map_err(|e| anyhow::anyhow!("S3 endpoint is not a URL: {e}"))?;
    anyhow::ensure!(matches!(url.scheme(), "http" | "https"), "S3 endpoint must use http or https");
    anyhow::ensure!(
        url.username().is_empty() && url.password().is_none(),
        "S3 endpoint must not carry credentials"
    );
    anyhow::ensure!(
        url.query().is_none() && url.fragment().is_none() && url.path() == "/",
        "S3 endpoint must be scheme://host[:port] with no path"
    );
    let name = url.host_str().ok_or_else(|| anyhow::anyhow!("S3 endpoint has no host"))?;
    let host = url.port().map_or_else(|| name.to_owned(), |port| format!("{name}:{port}"));
    Ok((format!("{}://{host}", url.scheme()), host))
}

/// URL parsing drops `.` and `..` segments, which would send a path other than
/// the one signed.
fn check_key(key: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !key.is_empty()
            && key.len() <= MAX_KEY_BYTES
            && key.split('/').all(|segment| segment != "." && segment != ".."),
        "invalid S3 object key {key:?}"
    );
    Ok(())
}

fn transport(op: &str, key: &str, e: reqwest::Error) -> anyhow::Error {
    anyhow::Error::new(e.without_url()).context(format!("s3 {op} {key}"))
}

async fn failure(resp: reqwest::Response, op: &str, key: &str) -> anyhow::Error {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    xml_code(&body).map_or_else(
        || anyhow::anyhow!("s3 {op} {key}: {status}"),
        |code| anyhow::anyhow!("s3 {op} {key}: {status} {code}"),
    )
}

fn xml_code(xml: &str) -> Option<&str> {
    let start = xml.find("<Code>")? + "<Code>".len();
    let len = xml[start..].find("</Code>")?;
    let code = xml[start..start + len].trim();
    (!code.is_empty() && code.len() <= 64 && code.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then_some(code)
}

#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::body::{Body, Bytes};
    use axum::extract::State;
    use axum::http::{HeaderMap, Uri, header};
    use axum::response::{IntoResponse, Response};
    use axum::routing::any;

    const KEY_ID: &str = "AKIDS3TEST";
    const SECRET: &str = "s3-secret-never-shown";
    const BUCKET: &str = "cctui-1";

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Seen {
        method: String,
        path: String,
        content_sha256: String,
    }

    #[derive(Clone, Default)]
    struct Fake {
        objects: Arc<Mutex<HashMap<String, Vec<u8>>>>,
        seen: Arc<Mutex<Vec<Seen>>>,
    }

    fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
        headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default()
    }

    /// Recomputes the signature from what arrived on the wire, so a path that
    /// differs from the signed one is rejected like a real S3 would.
    fn signature_matches(method: &str, path: &str, headers: &HeaderMap) -> bool {
        let amz_date = header_str(headers, "x-amz-date");
        let Ok(date) = chrono::NaiveDateTime::parse_from_str(amz_date, "%Y%m%dT%H%M%SZ") else {
            return false;
        };
        let now: SystemTime = date.and_utc().into();
        let payload = header_str(headers, "x-amz-content-sha256");
        let signed_headers = [("x-amz-content-sha256", payload)];
        let req = Request {
            method,
            host: header_str(headers, "host"),
            path,
            query: "",
            headers: &signed_headers,
            body: b"",
        };
        let creds = AwsCredentials::new(KEY_ID, SECRET);
        let scope = Scope { region: "us-east-1", service: "s3" };
        let want = sigv4::sign_with_payload_hash(&creds, scope, &req, payload, now);
        want.iter().any(|(k, v)| *k == "authorization" && v == header_str(headers, "authorization"))
    }

    async fn handle(
        State(fake): State<Fake>,
        method: axum::http::Method,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let path = uri.path().to_owned();
        let content_sha256 = header_str(&headers, "x-amz-content-sha256").to_owned();
        fake.seen.lock().unwrap().push(Seen {
            method: method.to_string(),
            path: path.clone(),
            content_sha256: content_sha256.clone(),
        });
        if !signature_matches(method.as_str(), &path, &headers) {
            return (StatusCode::FORBIDDEN, "<Error><Code>SignatureDoesNotMatch</Code></Error>")
                .into_response();
        }
        let Some(key) = path.strip_prefix(&format!("/{BUCKET}/")).map(str::to_owned) else {
            return (StatusCode::NOT_FOUND, "<Error><Code>NoSuchBucket</Code></Error>")
                .into_response();
        };
        let mut objects = fake.objects.lock().unwrap();
        match method.as_str() {
            "PUT" => {
                if content_sha256 != sha256_hex(&body) {
                    return (StatusCode::BAD_REQUEST, "<Error><Code>BadDigest</Code></Error>")
                        .into_response();
                }
                objects.insert(key, body.to_vec());
                StatusCode::OK.into_response()
            }
            "GET" | "HEAD" if content_sha256 != UNSIGNED_PAYLOAD => {
                StatusCode::BAD_REQUEST.into_response()
            }
            "GET" => objects.get(&key).map_or_else(
                || (StatusCode::NOT_FOUND, "<Error><Code>NoSuchKey</Code></Error>").into_response(),
                |bytes| bytes.clone().into_response(),
            ),
            "HEAD" => objects.get(&key).map_or_else(
                || StatusCode::NOT_FOUND.into_response(),
                |bytes| {
                    let mut resp = Response::new(Body::empty());
                    resp.headers_mut().insert(header::CONTENT_LENGTH, bytes.len().into());
                    resp
                },
            ),
            "DELETE" => {
                if objects.remove(&key).is_some() {
                    StatusCode::NO_CONTENT.into_response()
                } else {
                    StatusCode::NOT_FOUND.into_response()
                }
            }
            _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
        }
    }

    async fn serve(fake: Fake) -> String {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let app = Router::new().route("/{*path}", any(handle)).with_state(fake);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn client(endpoint: &str, secret: &str) -> S3Client {
        S3Client::new(endpoint, BUCKET, "us-east-1", AwsCredentials::new(KEY_ID, secret)).unwrap()
    }

    #[tokio::test]
    async fn put_head_get_delete_round_trip_against_a_fake_s3() {
        let fake = Fake::default();
        let endpoint = serve(fake.clone()).await;
        let s3 = client(&endpoint, SECRET);
        let bytes = b"blob bytes".to_vec();
        let key = sha256_hex(&bytes);

        assert_eq!(s3.head_object(&key).await.unwrap(), None);
        assert_eq!(s3.get_object(&key).await.unwrap(), None);
        s3.put_object(&key, &bytes, &key).await.unwrap();
        assert_eq!(s3.head_object(&key).await.unwrap(), Some(bytes.len() as u64));
        assert_eq!(s3.get_object(&key).await.unwrap(), Some(bytes));
        s3.delete_object(&key).await.unwrap();
        s3.delete_object(&key).await.unwrap();
        assert_eq!(s3.get_object(&key).await.unwrap(), None);

        let seen = fake.seen.lock().unwrap().clone();
        let path = format!("/{BUCKET}/{key}");
        let expect = |method: &str, sha: &str| Seen {
            method: method.to_owned(),
            path: path.clone(),
            content_sha256: sha.to_owned(),
        };
        assert_eq!(
            seen,
            vec![
                expect("HEAD", UNSIGNED_PAYLOAD),
                expect("GET", UNSIGNED_PAYLOAD),
                expect("PUT", &key),
                expect("HEAD", UNSIGNED_PAYLOAD),
                expect("GET", UNSIGNED_PAYLOAD),
                expect("DELETE", UNSIGNED_PAYLOAD),
                expect("DELETE", UNSIGNED_PAYLOAD),
                expect("GET", UNSIGNED_PAYLOAD),
            ]
        );
        assert_eq!(fake.objects.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn keys_with_reserved_bytes_are_signed_as_sent() {
        let fake = Fake::default();
        let s3 = client(&serve(fake.clone()).await, SECRET);
        let key = "prefix/a b+c%2Fd";
        s3.put_object(key, b"x", &sha256_hex(b"x")).await.unwrap();
        assert_eq!(s3.get_object(key).await.unwrap(), Some(b"x".to_vec()));
        assert!(fake.objects.lock().unwrap().contains_key("prefix/a%20b%2Bc%252Fd"));
        for bad in ["", "a/./b", "../escape", "a/.."] {
            assert!(s3.get_object(bad).await.is_err(), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn failures_never_leak_the_secret() {
        let fake = Fake::default();
        let endpoint = serve(fake.clone()).await;
        let wrong = client(&endpoint, "wrong-secret-value");
        let err = wrong.put_object("k", b"x", &sha256_hex(b"x")).await.unwrap_err();
        let shown = format!("{err:#} {err:?} {wrong:?}");
        assert!(shown.contains("403") && shown.contains("SignatureDoesNotMatch"), "{shown}");
        assert!(!shown.contains("wrong-secret-value"), "{shown}");
        assert!(!shown.contains("Signature="), "{shown}");

        let s3 = client(&endpoint, SECRET);
        let err = s3.put_object("k", b"x", &sha256_hex(b"y")).await.unwrap_err();
        assert!(format!("{err:#}").contains("BadDigest"), "{err:#}");
        assert!(fake.objects.lock().unwrap().is_empty());

        let down = S3Client::with_timeout(
            "http://127.0.0.1:1",
            BUCKET,
            "us-east-1",
            AwsCredentials::new(KEY_ID, SECRET),
            Duration::from_secs(2),
        )
        .unwrap();
        let err = format!("{:#}", down.head_object("k").await.unwrap_err());
        assert!(err.starts_with("s3 HEAD k"), "{err}");
        assert!(!err.contains(SECRET), "{err}");
    }

    #[test]
    fn endpoint_must_be_a_bare_origin() {
        let gw = "versitygw.storage.svc:7070";
        assert_eq!(
            parse_endpoint(&format!("http://{gw}")).unwrap(),
            (format!("http://{gw}"), gw.to_owned())
        );
        assert_eq!(
            parse_endpoint("https://s3.example.com:443/").unwrap(),
            ("https://s3.example.com".to_owned(), "s3.example.com".to_owned())
        );
        for bad in
            ["versitygw:7070", "ftp://h/", "http://user:pass@h/", "http://h/base", "http://h/?x=1"]
        {
            assert!(parse_endpoint(bad).is_err(), "{bad}");
        }
        let creds = AwsCredentials::new(KEY_ID, SECRET);
        assert!(S3Client::new("http://h", "a/b", "us-east-1", creds.clone()).is_err());
        assert!(S3Client::new("http://h", "", "us-east-1", creds).is_err());
    }
}
