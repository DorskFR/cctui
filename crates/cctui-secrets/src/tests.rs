use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

fn env_ref(var: &str) -> SecretRef {
    SecretRef::Env { var: var.to_owned() }
}

struct MockResolver {
    response: StdMutex<Result<String, &'static str>>,
    calls: AtomicUsize,
}

impl MockResolver {
    fn ok(value: &str) -> Self {
        Self { response: StdMutex::new(Ok(value.to_owned())), calls: AtomicUsize::new(0) }
    }

    fn failing(msg: &'static str) -> Self {
        Self { response: StdMutex::new(Err(msg)), calls: AtomicUsize::new(0) }
    }
}

#[async_trait::async_trait]
impl RefResolver for MockResolver {
    async fn resolve(&self, _r: &SecretRef) -> Result<Credential, SecretError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match &*self.response.lock().unwrap() {
            Ok(v) => Ok(Credential::new(v.clone())),
            Err(msg) => Err(SecretError::Backend(anyhow::anyhow!(*msg))),
        }
    }
}

struct ArcResolver(std::sync::Arc<MockResolver>);

#[async_trait::async_trait]
impl RefResolver for ArcResolver {
    async fn resolve(&self, r: &SecretRef) -> Result<Credential, SecretError> {
        self.0.resolve(r).await
    }
}

#[tokio::test(start_paused = true)]
async fn cache_serves_fresh_hit_without_engine_call() {
    let mock = std::sync::Arc::new(MockResolver::ok("s3cret"));
    let source = SecretSource::new(Box::new(ArcResolver(mock.clone())), Duration::from_mins(2));
    let r = env_ref("CRED_X");
    assert_eq!(source.fetch(&r).await.unwrap().expose(), "s3cret");
    tokio::time::advance(Duration::from_secs(119)).await;
    assert_eq!(source.fetch(&r).await.unwrap().expose(), "s3cret");
    assert_eq!(mock.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn cache_expiry_refetches_and_picks_up_rotation() {
    let mock = std::sync::Arc::new(MockResolver::ok("v1"));
    let source = SecretSource::new(Box::new(ArcResolver(mock.clone())), Duration::from_mins(2));
    let r = env_ref("CRED_X");
    assert_eq!(source.fetch(&r).await.unwrap().expose(), "v1");

    *mock.response.lock().unwrap() = Ok("v2".to_owned());
    tokio::time::advance(Duration::from_secs(119)).await;
    assert_eq!(
        source.fetch(&r).await.unwrap().expose(),
        "v1",
        "rotation must NOT land before TTL expiry"
    );
    assert_eq!(mock.calls.load(Ordering::SeqCst), 1);

    tokio::time::advance(Duration::from_secs(2)).await;
    assert_eq!(
        source.fetch(&r).await.unwrap().expose(),
        "v2",
        "rotation must land after TTL expiry"
    );
    assert_eq!(mock.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn engine_error_fails_closed_and_is_not_cached() {
    let mock = std::sync::Arc::new(MockResolver::failing("boom"));
    let source = SecretSource::new(Box::new(ArcResolver(mock.clone())), Duration::from_mins(2));
    let r = env_ref("CRED_X");
    assert!(matches!(source.fetch(&r).await, Err(SecretError::Backend(_))));
    *mock.response.lock().unwrap() = Ok("recovered".to_owned());
    assert_eq!(source.fetch(&r).await.unwrap().expose(), "recovered");
    assert_eq!(mock.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn engine_error_past_ttl_drops_stale_entry() {
    let mock = std::sync::Arc::new(MockResolver::ok("v1"));
    let source = SecretSource::new(Box::new(ArcResolver(mock.clone())), Duration::from_mins(2));
    let r = env_ref("CRED_X");
    assert_eq!(source.fetch(&r).await.unwrap().expose(), "v1");
    *mock.response.lock().unwrap() = Err("engine down");
    tokio::time::advance(Duration::from_secs(121)).await;
    assert!(matches!(source.fetch(&r).await, Err(SecretError::Backend(_))));
}

#[test]
fn credential_debug_is_redacted() {
    let c = Credential::new("hunter2".to_owned());
    assert_eq!(format!("{c:?}"), "Credential(<redacted>)");
}

#[test]
fn ref_parse_roundtrip() {
    for (input, display) in [
        ("env:GITHUB_TOKEN", "env:GITHUB_TOKEN"),
        ("file:/run/secrets/key", "file:/run/secrets/key"),
        ("file:relative/key", "file:relative/key"),
        (
            "vault:kvmount/data/cctui/workers#GITHUB_TOKEN_ACME",
            "vault:kvmount/data/cctui/workers#GITHUB_TOKEN_ACME",
        ),
        ("vault:kv/data/a/b/c#f", "vault:kv/data/a/b/c#f"),
        ("aws-sm:cctui/worker/acme/github", "aws-sm:cctui/worker/acme/github"),
        ("aws-sm:cctui/worker#token", "aws-sm:cctui/worker#token"),
        ("k8s:my-secret#token", "k8s:my-secret#token"),
        ("k8s:dev/my-secret#token", "k8s:dev/my-secret#token"),
        ("bao:kvmount/data/cctui/workers#GPG_KEY", "vault:kvmount/data/cctui/workers#GPG_KEY"),
    ] {
        let r = SecretRef::parse(input).unwrap();
        assert_eq!(r.to_string(), display, "roundtrip of {input}");
        assert_eq!(SecretRef::parse(&r.to_string()).unwrap(), r);
    }
}

#[test]
fn ref_parse_rejects_malformed() {
    for bad in [
        "",
        "GITHUB_TOKEN",
        "env:",
        "file:",
        "vault:kvmount#F",
        "vault:kvmount/data/x",
        "vault:/data/x#f",
        "vault:m/data/#f",
        "vault:m/data/p#",
        "aws-sm:",
        "aws-sm:name#",
        "k8s:secret",
        "k8s:#key",
        "k8s:/secret#key",
        "s3:bucket/key",
    ] {
        assert!(SecretRef::parse(bad).is_err(), "{bad:?} must be rejected");
    }
}

#[tokio::test]
async fn env_engine_resolves_and_fails_closed() {
    let engines = Engines::new().with_env_lookup(Box::new(|name| match name {
        "CRED_ACME_GITHUB" => Some("tok".to_owned()),
        "CRED_ACME_EMPTY" => Some(String::new()),
        _ => None,
    }));
    assert_eq!(engines.resolve(&env_ref("CRED_ACME_GITHUB")).await.unwrap().expose(), "tok");
    assert!(matches!(
        engines.resolve(&env_ref("CRED_ACME_EMPTY")).await,
        Err(SecretError::NotFound { .. })
    ));
    assert!(matches!(
        engines.resolve(&env_ref("CRED_ACME_MISSING")).await,
        Err(SecretError::NotFound { .. })
    ));
}

#[tokio::test]
async fn unconfigured_engines_fail_closed_as_backend() {
    let engines = Engines::new();
    let vault = SecretRef::parse("vault:m/data/p#f").unwrap();
    assert!(matches!(engines.resolve(&vault).await, Err(SecretError::Backend(_))));
    let k8s = SecretRef::parse("k8s:sec#key").unwrap();
    assert!(matches!(engines.resolve(&k8s).await, Err(SecretError::Backend(_))));
}

fn file_ref(path: &std::path::Path) -> SecretRef {
    SecretRef::parse(&format!("file:{}", path.display())).unwrap()
}

#[tokio::test]
async fn file_engine_reads_and_trims_one_trailing_newline() {
    let dir = tempfile::tempdir().unwrap();
    let engines = Engines::new();
    for (contents, want) in [
        ("s3cret\n", "s3cret"),
        ("s3cret\r\n", "s3cret"),
        ("s3cret", "s3cret"),
        ("two\n\n", "two\n"),
        ("  padded  \n", "  padded  "),
    ] {
        let path = dir.path().join("key");
        std::fs::write(&path, contents).unwrap();
        assert_eq!(engines.resolve(&file_ref(&path)).await.unwrap().expose(), want, "{contents:?}");
    }
}

#[tokio::test]
async fn file_engine_missing_or_empty_is_not_found_and_unreadable_is_backend() {
    let dir = tempfile::tempdir().unwrap();
    let engines = Engines::new();
    let missing = file_ref(&dir.path().join("missing"));
    assert!(matches!(engines.resolve(&missing).await, Err(SecretError::NotFound { .. })));
    let empty = dir.path().join("empty");
    std::fs::write(&empty, "\n").unwrap();
    assert!(matches!(engines.resolve(&file_ref(&empty)).await, Err(SecretError::NotFound { .. })));
    assert!(matches!(engines.resolve(&file_ref(dir.path())).await, Err(SecretError::Backend(_))));
}

#[test]
fn secret_value_only_known_schemes_are_refs() {
    for literal in [
        "postgres://user:pw@host/db",
        "postgresql://user:p%3Aw@host:5432/db?sslmode=require",
        "abc:def:ghi",
        "tok_en:with:colons",
        "https://example.test",
        "ENV:UPPER",
        "environment:x",
        "files:/x",
        "k8s",
        "plain-token",
        "",
    ] {
        assert_eq!(SecretValue::parse(literal).unwrap(), SecretValue::Literal(literal));
    }
    assert_eq!(SecretValue::parse("literal:env:X").unwrap(), SecretValue::Literal("env:X"));
    assert_eq!(SecretValue::parse("literal:").unwrap(), SecretValue::Literal(""));
    assert_eq!(SecretValue::parse("env:X").unwrap(), SecretValue::Ref(env_ref("X")));
    assert!(SecretValue::parse("k8s:missing-key").is_err(), "malformed ref is not a literal");
}

#[tokio::test]
async fn resolve_value_passes_literals_through_and_resolves_refs() {
    let engines = Engines::new().with_env_lookup(Box::new(|name| {
        (name == "DB_PASSWORD_URL").then(|| "postgres://app:s3cret@db/app".to_owned())
    }));
    for literal in ["postgres://user:pw@host/db", "a:b:c", "plain"] {
        assert_eq!(resolve_value(&engines, literal).await.unwrap(), literal);
    }
    assert_eq!(resolve_value(&engines, "literal:file:/x").await.unwrap(), "file:/x");
    assert_eq!(
        resolve_value(&engines, "env:DB_PASSWORD_URL").await.unwrap(),
        "postgres://app:s3cret@db/app"
    );
}

#[tokio::test]
async fn resolve_value_error_names_the_ref_not_the_value() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing");
    let raw = format!("file:{}", path.display());
    let msg = resolve_value(&Engines::new(), &raw).await.unwrap_err().to_string();
    assert!(msg.contains(&raw), "{msg}");

    let engines = Engines::new().with_env_lookup(Box::new(|_| Some(String::new())));
    let msg = resolve_value(&engines, "env:EMPTY_ONE").await.unwrap_err().to_string();
    assert!(msg.contains("env:EMPTY_ONE"), "{msg}");
}

#[test]
fn resolve_blocking_reads_a_file_ref_outside_a_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("key");
    std::fs::write(&path, "dk-123\n").unwrap();
    assert_eq!(resolve_blocking(&format!("file:{}", path.display())).unwrap(), "dk-123");
    let url = "postgres://user:pw@host/db";
    assert_eq!(resolve_blocking(url).unwrap(), url);
}

#[tokio::test]
async fn resolve_blocking_works_inside_a_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("key");
    std::fs::write(&path, "dk-456\n").unwrap();
    assert_eq!(resolve_blocking(&format!("file:{}", path.display())).unwrap(), "dk-456");
}

#[test]
fn resolve_env_blocking_resolves_refs_keeps_literals_and_skips_unset() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("token");
    std::fs::write(&path, "admin-tok\n").unwrap();
    let token_ref = format!("file:{}", path.display());
    let got = resolve_env_blocking(&["DATABASE_URL", "CCTUI_ADMIN_TOKENS", "UNSET"], |k| match k {
        "DATABASE_URL" => Some("postgres://user:pw@host/db".to_owned()),
        "CCTUI_ADMIN_TOKENS" => Some(token_ref.clone()),
        _ => None,
    })
    .unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got["DATABASE_URL"], "postgres://user:pw@host/db");
    assert_eq!(got["CCTUI_ADMIN_TOKENS"], "admin-tok");
}

#[test]
fn resolve_env_blocking_error_names_the_var_and_ref() {
    let dir = tempfile::tempdir().unwrap();
    let raw = format!("file:{}", dir.path().join("missing").display());
    let msg = resolve_env_blocking(&["CCTUI_ADMIN_TOKENS"], |_| Some(raw.clone()))
        .unwrap_err()
        .to_string();
    assert!(msg.contains("CCTUI_ADMIN_TOKENS"), "{msg}");
    assert!(msg.contains(&raw), "{msg}");
}
