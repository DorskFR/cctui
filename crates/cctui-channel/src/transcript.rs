//! Tail a Claude Code `session.jsonl`, forwarding each raw JSONL line to a
//! callback. Matches the behaviour of `channel/src/transcript.ts`:
//!   * up to 30 s wait for the file to appear,
//!   * byte-offset tracking (re-read from zero on truncation),
//!   * 300 ms poll loop,
//!   * trim + skip empty lines.
//!
//! When an `offset_path` is provided the byte offset is persisted across
//! channel restarts so a Claude Code restart on the same session does not
//! replay the entire transcript (CCT-66).

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::fs;
use tokio::sync::mpsc;

pub async fn tail(
    transcript_path: PathBuf,
    offset_path: Option<PathBuf>,
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

    let mut offset: u64 = load_offset(offset_path.as_deref()).await;

    // If the persisted offset is past the current file size (file shrank /
    // rotated while the channel was down) fall back to the start.
    if let Ok(meta) = fs::metadata(&transcript_path).await
        && meta.len() < offset
    {
        offset = 0;
    }

    loop {
        if *cancel.borrow() {
            return;
        }
        read_new(&transcript_path, &mut offset, &tx).await;
        save_offset(offset_path.as_deref(), offset).await;
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

async fn load_offset(path: Option<&Path>) -> u64 {
    let Some(path) = path else { return 0 };
    match fs::read_to_string(path).await {
        Ok(s) => s.trim().parse().unwrap_or(0),
        Err(_) => 0,
    }
}

async fn save_offset(path: Option<&Path>, offset: u64) {
    let Some(path) = path else { return };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent).await;
    }
    // Best-effort — a failed save just means we'll replay a few lines on
    // the next restart, which is recoverable via server-side dedup of
    // identical consecutive lines.
    let _ = fs::write(path, offset.to_string()).await;
}
