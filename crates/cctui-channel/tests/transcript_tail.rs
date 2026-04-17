//! Transcript tailer — minimal edge cases from `channel/test/transcript.test.ts`.

use std::time::Duration;

use cctui_channel::transcript;
use tempfile::tempdir;
use tokio::sync::{mpsc, watch};

async fn collect(mut rx: mpsc::Receiver<String>, timeout: Duration) -> Vec<String> {
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(line)) => out.push(line),
            Ok(None) | Err(_) => break,
        }
    }
    out
}

#[tokio::test]
async fn reads_existing_lines_and_trims_blanks() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    std::fs::write(&path, "a\n\nb\n").unwrap();
    let (tx, rx) = mpsc::channel(16);
    let (_ctx, crx) = watch::channel(false);
    tokio::spawn(transcript::tail(path, tx, crx));
    let lines = collect(rx, Duration::from_millis(1200)).await;
    assert!(lines.contains(&"a".to_string()));
    assert!(lines.contains(&"b".to_string()));
    assert!(!lines.contains(&String::new()));
}

#[tokio::test]
async fn picks_up_appends() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    std::fs::write(&path, "first\n").unwrap();
    let (tx, rx) = mpsc::channel(16);
    let (_ctx, crx) = watch::channel(false);
    let path_c = path.clone();
    tokio::spawn(transcript::tail(path_c, tx, crx));
    tokio::time::sleep(Duration::from_millis(500)).await;
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"second\n").unwrap();
    }
    let lines = collect(rx, Duration::from_millis(1500)).await;
    assert!(lines.contains(&"first".to_string()));
    assert!(lines.contains(&"second".to_string()));
}

#[tokio::test]
async fn rereads_after_truncation() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    std::fs::write(&path, "old1\nold2\n").unwrap();
    let (tx, rx) = mpsc::channel(16);
    let (_ctx, crx) = watch::channel(false);
    tokio::spawn(transcript::tail(path.clone(), tx, crx));
    tokio::time::sleep(Duration::from_millis(500)).await;
    std::fs::write(&path, "brand\n").unwrap();
    let lines = collect(rx, Duration::from_millis(1500)).await;
    assert!(lines.contains(&"brand".to_string()));
}
