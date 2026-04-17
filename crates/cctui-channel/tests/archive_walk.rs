//! Archive walker + sha256 — port of `channel/test/archive.test.ts`.

use cctui_channel::archive;
use tempfile::tempdir;

#[test]
fn walks_project_dirs_and_skips_non_jsonl() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let proj = root.join("proj-a");
    std::fs::create_dir(&proj).unwrap();
    std::fs::write(proj.join("a.jsonl"), b"{}\n").unwrap();
    std::fs::write(proj.join("b.txt"), b"ignored").unwrap();

    let files = archive::walk_project_dirs(root);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].session_id, "a");
    assert_eq!(files[0].project_dir, "proj-a");
}

#[test]
fn sha256_is_stable() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("f");
    std::fs::write(&p, b"hello\n").unwrap();
    let sha = archive::compute_file_sha256(&p).unwrap();
    assert_eq!(sha, "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03");
}

#[test]
fn missing_root_returns_empty() {
    let dir = tempdir().unwrap();
    let nowhere = dir.path().join("does-not-exist");
    assert!(archive::walk_project_dirs(&nowhere).is_empty());
}
