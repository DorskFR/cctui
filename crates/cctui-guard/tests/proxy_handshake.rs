use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use cctui_guard::engine::WorkflowEngine;
use cctui_guard::ir::Workflow;
use cctui_guard::parser::parse_guard_rules_str;
use serde_json::{Value, json};

const RULES: &str = "[net-claude]: api.example.com:443\n[net-github]: github.example.com:443\n";

const PROMPT: &str = "\
# Step 1: Research

[allowed]: Read
[network]: net-claude
[transition]: 2, Exit

# Step 2: Publish

[allowed]: Read
[network]: net-claude, net-github
[transition]: Exit
";

struct StubProxy {
    addr: String,
    reported: Arc<AtomicU64>,
    hits: Arc<AtomicU64>,
}

fn stub_proxy(body: fn(u64) -> String) -> StubProxy {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let reported = Arc::new(AtomicU64::new(0));
    let hits = Arc::new(AtomicU64::new(0));
    let (r, h) = (reported.clone(), hits.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            h.fetch_add(1, Ordering::SeqCst);
            let body = body(r.load(Ordering::SeqCst));
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    StubProxy { addr, reported, hits }
}

fn generation_body(generation: u64) -> String {
    json!({ "ready": true, "generation": generation }).to_string()
}

fn engine(dir: &Path, ready: Option<String>, timeout: Duration) -> (Arc<WorkflowEngine>, PathBuf) {
    let proxy_dir = dir.join("guard-proxy");
    std::fs::create_dir_all(&proxy_dir).unwrap();
    let policy = proxy_dir.join("policy.json");
    let engine = WorkflowEngine::new(
        Workflow::compile(PROMPT).unwrap().into_steps(),
        parse_guard_rules_str(RULES),
        dir.join("state"),
        policy.clone(),
        vec![],
        dir.to_path_buf(),
        None,
        false,
    )
    .with_proxy_handshake(ready, timeout);
    (Arc::new(engine), policy)
}

fn written_generation(policy: &Path) -> u64 {
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(policy).unwrap()).unwrap();
    doc["generation"].as_u64().unwrap()
}

#[test]
fn policy_writes_carry_an_increasing_generation() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, policy) = engine(dir.path(), None, Duration::from_secs(2));
    let first = written_generation(&policy);
    assert!(first >= 1);
    assert_eq!(engine.transition(&json!(2))["ok"], true);
    assert_eq!(written_generation(&policy), first + 1);
    assert_eq!(engine.transition(&json!("exit"))["ok"], true);
    assert_eq!(written_generation(&policy), first + 2);
}

#[test]
fn a_restarted_guard_continues_the_generation_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let (first, policy) = engine(dir.path(), None, Duration::from_secs(2));
    assert_eq!(first.transition(&json!(2))["ok"], true);
    let before = written_generation(&policy);
    drop(first);
    let (_restarted, policy) = engine(dir.path(), None, Duration::from_secs(2));
    assert!(written_generation(&policy) > before);
}

#[test]
fn transition_waits_until_the_proxy_reports_the_new_generation() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = stub_proxy(generation_body);
    let (engine, policy) = engine(dir.path(), Some(proxy.addr.clone()), Duration::from_secs(5));
    proxy.reported.store(written_generation(&policy), Ordering::SeqCst);

    let done = Arc::new(AtomicBool::new(false));
    let worker = {
        let done = done.clone();
        std::thread::spawn(move || {
            let resp = engine.transition(&json!(2));
            done.store(true, Ordering::SeqCst);
            resp
        })
    };

    std::thread::sleep(Duration::from_millis(400));
    assert!(!done.load(Ordering::SeqCst), "/transition answered before the proxy reloaded");
    assert!(proxy.hits.load(Ordering::SeqCst) > 0, "the proxy was never asked");

    let confirmed_at = Instant::now();
    proxy.reported.store(written_generation(&policy), Ordering::SeqCst);
    let resp = worker.join().unwrap();
    assert_eq!(resp["ok"], true, "{resp}");
    assert!(confirmed_at.elapsed() < Duration::from_secs(1));
}

#[test]
fn transition_answers_after_the_timeout_when_the_generation_never_matches() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = stub_proxy(generation_body);
    let timeout = Duration::from_millis(300);
    let (engine, _policy) = engine(dir.path(), Some(proxy.addr.clone()), timeout);

    let started = Instant::now();
    let resp = engine.transition(&json!(2));
    let waited = started.elapsed();
    assert_eq!(resp["ok"], true, "{resp}");
    assert!(waited >= timeout, "answered after {waited:?}");
    assert!(waited < Duration::from_secs(2), "answered after {waited:?}");
    assert!(proxy.hits.load(Ordering::SeqCst) > 1);
}

#[test]
fn transition_does_not_wait_on_a_proxy_without_generation_support() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = stub_proxy(|_| "Ready".to_string());
    let (engine, _policy) = engine(dir.path(), Some(proxy.addr.clone()), Duration::from_secs(5));

    let started = Instant::now();
    assert_eq!(engine.transition(&json!(2))["ok"], true);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(proxy.hits.load(Ordering::SeqCst), 1);
}

#[test]
fn transition_does_not_wait_on_an_unreachable_proxy() {
    let dir = tempfile::tempdir().unwrap();
    let addr = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().to_string()
    };
    let (engine, _policy) = engine(dir.path(), Some(addr), Duration::from_secs(5));

    let started = Instant::now();
    assert_eq!(engine.transition(&json!(2))["ok"], true);
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn a_refused_transition_does_not_wait_on_the_proxy() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = stub_proxy(generation_body);
    let (engine, _policy) = engine(dir.path(), Some(proxy.addr.clone()), Duration::from_secs(5));

    assert_eq!(engine.transition(&json!(7))["ok"], false);
    assert_eq!(proxy.hits.load(Ordering::SeqCst), 0);
}
