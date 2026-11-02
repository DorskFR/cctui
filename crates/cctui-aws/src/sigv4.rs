//! [`Request`] takes the path and query in canonical form: see [`canonical_uri`]
//! and [`canonical_query`].

use std::fmt::Write as _;
use std::time::SystemTime;

use hmac::{Hmac, Mac as _};
use sha2::{Digest as _, Sha256};

use crate::AwsCredentials;

const ALGORITHM: &str = "AWS4-HMAC-SHA256";

#[derive(Debug, Clone, Copy)]
pub struct Scope<'a> {
    pub region: &'a str,
    pub service: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    pub method: &'a str,
    /// Must equal the `Host` header sent, port included when not the default.
    pub host: &'a str,
    pub path: &'a str,
    pub query: &'a str,
    /// Extra headers to sign. `host`, `x-amz-date` and `x-amz-security-token`
    /// are added by the signer.
    pub headers: &'a [(&'a str, &'a str)],
    pub body: &'a [u8],
}

/// Signs `req` and returns the headers to add to it: `x-amz-date`,
/// `x-amz-security-token` for temporary credentials, and `authorization`.
#[must_use]
pub fn sign(
    creds: &AwsCredentials,
    scope: Scope<'_>,
    req: &Request<'_>,
    now: SystemTime,
) -> Vec<(&'static str, String)> {
    let amz_date = amz_date(now);
    let headers = canonical_headers(creds, req, &amz_date);
    let signed = signed_headers(&headers);
    let canonical = canonical_request(req, &headers, &signed);
    let credential_scope = credential_scope(&amz_date, scope);
    let to_sign = string_to_sign(&amz_date, &credential_scope, &canonical);
    let signature = signature(&creds.secret_key, &amz_date, scope, &to_sign);
    let mut out = vec![("x-amz-date", amz_date)];
    if let Some(token) = &creds.session_token {
        out.push(("x-amz-security-token", token.clone()));
    }
    out.push((
        "authorization",
        format!(
            "{ALGORITHM} Credential={}/{credential_scope}, SignedHeaders={signed}, \
             Signature={signature}",
            creds.key_id
        ),
    ));
    out
}

/// Canonical URI of a request path as sent on the wire: dot segments and
/// empty segments removed, then every byte outside the unreserved set and `/`
/// percent-encoded, so an existing `%XX` is encoded again.
#[must_use]
pub fn canonical_uri(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            s => segments.push(s),
        }
    }
    let mut out = String::from("/");
    out.push_str(&segments.join("/"));
    let trailing = ["/", "/.", "/..", "/./", "/../"].iter().any(|s| path.ends_with(s));
    if trailing && !out.ends_with('/') {
        out.push('/');
    }
    uri_encode(&out, true)
}

/// Canonical query string: pairs form-decoded, re-encoded, then sorted.
#[must_use]
pub fn canonical_query(query: &str) -> String {
    let mut pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (uri_encode(&form_decode(k), false), uri_encode(&form_decode(v), false))
        })
        .collect();
    pairs.sort();
    pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")
}

fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
        if unreserved || (keep_slash && b == b'/') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

fn form_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex_pair = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex_pair.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(decoded)) => {
                out.push(decoded);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn amz_date(now: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(now).format("%Y%m%dT%H%M%SZ").to_string()
}

fn canonical_headers(
    creds: &AwsCredentials,
    req: &Request<'_>,
    amz_date: &str,
) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = req
        .headers
        .iter()
        .map(|(k, v)| {
            (k.trim().to_ascii_lowercase(), v.split_whitespace().collect::<Vec<_>>().join(" "))
        })
        .collect();
    headers.push(("host".to_owned(), req.host.to_owned()));
    headers.push(("x-amz-date".to_owned(), amz_date.to_owned()));
    if let Some(token) = &creds.session_token {
        headers.push(("x-amz-security-token".to_owned(), token.clone()));
    }
    headers.sort();
    headers
}

fn signed_headers(headers: &[(String, String)]) -> String {
    headers.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(";")
}

fn canonical_request(req: &Request<'_>, headers: &[(String, String)], signed: &str) -> String {
    let mut lines = String::new();
    for (k, v) in headers {
        let _ = writeln!(lines, "{k}:{v}");
    }
    format!(
        "{}\n{}\n{}\n{lines}\n{signed}\n{}",
        req.method,
        req.path,
        req.query,
        hex::encode(Sha256::digest(req.body))
    )
}

fn credential_scope(amz_date: &str, scope: Scope<'_>) -> String {
    format!("{}/{}/{}/aws4_request", &amz_date[..8], scope.region, scope.service)
}

fn string_to_sign(amz_date: &str, credential_scope: &str, canonical: &str) -> String {
    format!(
        "{ALGORITHM}\n{amz_date}\n{credential_scope}\n{}",
        hex::encode(Sha256::digest(canonical.as_bytes()))
    )
}

fn signature(secret_key: &str, amz_date: &str, scope: Scope<'_>, to_sign: &str) -> String {
    let key = hmac_sha256(format!("AWS4{secret_key}").as_bytes(), &amz_date.as_bytes()[..8]);
    let key = hmac_sha256(&key, scope.region.as_bytes());
    let key = hmac_sha256(&key, scope.service.as_bytes());
    let key = hmac_sha256(&key, b"aws4_request");
    hex::encode(hmac_sha256(&key, to_sign.as_bytes()))
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_ID: &str = "AKIDEXAMPLE";
    const SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
    const TOKEN: &str = "6e86291e8372ff2a2260956d9b8aae1d763fbf315fa00fa31553b73ebf194267";
    const SCOPE: Scope<'static> = Scope { region: "us-east-1", service: "service" };

    fn clock() -> SystemTime {
        chrono::DateTime::parse_from_rfc3339("2015-08-30T12:36:00Z").unwrap().into()
    }

    fn request<'a>(
        method: &'a str,
        headers: &'a [(&'a str, &'a str)],
        body: &'a [u8],
    ) -> Request<'a> {
        Request { method, host: "example.amazonaws.com", path: "/", query: "", headers, body }
    }

    fn authorization(signed: &[(&'static str, String)]) -> &str {
        &signed.iter().find(|(k, _)| *k == "authorization").unwrap().1
    }

    #[test]
    fn get_vanilla_matches_the_aws_test_suite() {
        let creds = AwsCredentials::new(KEY_ID, SECRET);
        let req = request("GET", &[], b"");
        let headers = canonical_headers(&creds, &req, "20150830T123600Z");
        let canonical = canonical_request(&req, &headers, &signed_headers(&headers));
        assert_eq!(
            canonical,
            "GET\n/\n\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\nhost;x-amz-date\n\
             e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let scope = credential_scope("20150830T123600Z", SCOPE);
        assert_eq!(
            string_to_sign("20150830T123600Z", &scope, &canonical),
            "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/service/aws4_request\n\
             bb579772317eb040ac9ed261061d46c1f17a8133879d6129b6e1c25292927e63"
        );
        let signed = sign(&creds, SCOPE, &req, clock());
        assert_eq!(
            signed,
            vec![
                ("x-amz-date", "20150830T123600Z".to_owned()),
                (
                    "authorization",
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/\
                     aws4_request, SignedHeaders=host;x-amz-date, \
                     Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
                        .to_owned()
                ),
            ]
        );
    }

    #[test]
    fn post_vanilla_matches_the_aws_test_suite() {
        let creds = AwsCredentials::new(KEY_ID, SECRET);
        let signed = sign(&creds, SCOPE, &request("POST", &[], b""), clock());
        assert!(
            authorization(&signed).ends_with(
                "Signature=5da7c1a2acd57cee7505fc6676e4e544621c30862966e37dddb68e92efbe5d6b"
            ),
            "{signed:?}"
        );
    }

    #[test]
    fn session_token_is_sent_and_signed() {
        let mut creds = AwsCredentials::new(KEY_ID, SECRET);
        creds.session_token = Some(TOKEN.to_owned());
        let signed = sign(&creds, SCOPE, &request("GET", &[], b""), clock());
        assert!(signed.contains(&("x-amz-security-token", TOKEN.to_owned())), "{signed:?}");
        assert_eq!(
            authorization(&signed),
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=host;x-amz-date;x-amz-security-token, \
             Signature=07ec1639c89043aa0e3e2de82b96708f198cceab042d4a97044c66dd9f74e7f8"
        );
    }

    #[test]
    fn signed_body_and_extra_headers_match_the_aws_test_suite() {
        let creds = AwsCredentials::new(KEY_ID, SECRET);
        let body = b"Param1=value1";
        let headers = [
            ("Content-Type", "application/x-www-form-urlencoded"),
            ("Content-Length", "13"),
            (
                "x-amz-content-sha256",
                "9095672bbd1f56dfc5b65f3e153adc8731a4a654192329106275f4c7b24d0b6e",
            ),
        ];
        let signed = sign(&creds, SCOPE, &request("POST", &headers, body), clock());
        assert_eq!(
            authorization(&signed),
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=content-length;content-type;host;x-amz-content-sha256;x-amz-date, \
             Signature=d3875051da38690788ef43de4db0d8f280229d82040bfac253562e56c3f20e0b"
        );
    }

    #[test]
    fn canonical_uri_matches_the_aws_test_suite() {
        for (raw, canonical) in [
            ("", "/"),
            ("/", "/"),
            ("/./", "/"),
            ("/./example", "/example"),
            ("//example//", "/example/"),
            ("/example1/example2/../..", "/"),
            ("/ሴ", "/%E1%88%B4"),
            ("/test/@connections/JBDvjfGEIAMCERw%3D", "/test/%40connections/JBDvjfGEIAMCERw%253D"),
        ] {
            assert_eq!(canonical_uri(raw), canonical, "{raw:?}");
        }
    }

    #[test]
    fn canonical_query_matches_the_aws_test_suite() {
        let unreserved = "-._~0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        let unreserved_pair = format!("{unreserved}={unreserved}");
        for (raw, canonical) in [
            ("", ""),
            ("Param2=value2&Param1=value1", "Param1=value1&Param2=value2"),
            (
                "Param-3=Value3&Param=Value2&%E1%88%B4=Value1",
                "%E1%88%B4=Value1&Param=Value2&Param-3=Value3",
            ),
            ("ሴ=bar", "%E1%88%B4=bar"),
            (unreserved_pair.as_str(), unreserved_pair.as_str()),
            (
                "Action=GetCallerIdentity&Version=2011-06-15",
                "Action=GetCallerIdentity&Version=2011-06-15",
            ),
            ("b&a=1+2", "a=1%202&b="),
        ] {
            assert_eq!(canonical_query(raw), canonical, "{raw:?}");
        }
    }

    #[test]
    fn query_signature_matches_the_aws_test_suite() {
        let creds = AwsCredentials::new(KEY_ID, SECRET);
        let query = canonical_query("Param2=value2&Param1=value1");
        let req = Request { query: &query, ..request("GET", &[], b"") };
        assert!(
            authorization(&sign(&creds, SCOPE, &req, clock())).ends_with(
                "Signature=b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500"
            )
        );
    }

    #[test]
    fn header_values_are_trimmed_and_spaces_collapsed() {
        let creds = AwsCredentials::new(KEY_ID, SECRET);
        let req = request("GET", &[("My-Header1", "  a   b   c  ")], b"");
        let headers = canonical_headers(&creds, &req, "20150830T123600Z");
        assert_eq!(headers[0], ("my-header1".to_owned(), "a b c".to_owned()));
    }
}
