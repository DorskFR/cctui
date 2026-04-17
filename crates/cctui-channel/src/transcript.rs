//! Tail a Claude Code `session.jsonl`, forwarding each raw JSONL line to a
//! callback. Matches the behaviour of `channel/src/transcript.ts`:
//!   * up to 30 s wait for the file to appear,
//!   * byte-offset tracking (re-read from zero on truncation),
//!   * 300 ms poll loop,
//!   * trim + skip empty lines.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::fs;
use tokio::sync::mpsc;

pub async fn tail(
    transcript_path: PathBuf,
    tx: mpsc::Sender<String>,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) {
    // Wait up to 30 s for the file.
    for _ in 0..60 {
        if *cancel.borrow() {
            return;
        }
        if fs::metadata(&transcript_path).await.is_ok() {
            break;
        }
        tokio::select! {
            () = tokio::time::sleep(Duration::from_millis(500)) => {},
            _ = cancel.changed() => return,
        }
    }

    let mut offset: u64 = 0;

    loop {
        if *cancel.borrow() {
            return;
        }
        read_new(&transcript_path, &mut offset, &tx).await;
        tokio::select! {
            () = tokio::time::sleep(Duration::from_millis(300)) => {},
            _ = cancel.changed() => return,
        }
    }
}

async fn read_new(path: &Path, offset: &mut u64, tx: &mpsc::Sender<String>) {
    let Ok(meta) = fs::metadata(path).await else { return };
    let size = meta.len();
    if size < *offset {
        // Truncated / rotated — re-read from zero.
        *offset = 0;
    }
    if size <= *offset {
        return;
    }
    let Ok(bytes) = fs::read(path).await else { return };
    if bytes.len() as u64 <= *offset {
        return;
    }
    let new_slice = &bytes[*offset as usize..];
    *offset = bytes.len() as u64;
    let Ok(text) = std::str::from_utf8(new_slice) else { return };
    for line in text.split('\n') {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if tx.send(trimmed.to_string()).await.is_err() {
            return;
        }
    }
}
