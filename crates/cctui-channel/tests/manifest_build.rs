use std::fs;
use std::io::Write;

use cctui_channel::manifest;

#[test]
fn build_captures_jsonl_files_with_size_and_mtime() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let proj = root.join("-home-user-foo");
    fs::create_dir_all(&proj).unwrap();

    let session_path = proj.join("11111111-2222-3333-4444-555555555555.jsonl");
    let mut f = fs::File::create(&session_path).unwrap();
    writeln!(f, "{{\"type\":\"dummy\"}}").unwrap();
    f.sync_all().unwrap();
    let expected_size = i64::try_from(fs::metadata(&session_path).unwrap().len()).unwrap();

    // Non-jsonl should be ignored.
    fs::File::create(proj.join("other.txt")).unwrap();

    let entries = manifest::build(root);

    assert_eq!(entries.len(), 1, "only the jsonl should be captured");
    let e = &entries[0];
    assert_eq!(e.project_dir, "-home-user-foo");
    assert_eq!(e.session_id, "11111111-2222-3333-4444-555555555555");
    assert_eq!(e.size_bytes, expected_size);
    // mtime is the file's modification time, so it's in the past-or-now.
    assert!(e.mtime <= chrono::Utc::now());
}

#[test]
fn empty_root_returns_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let entries = manifest::build(tmp.path());
    assert!(entries.is_empty());
}
