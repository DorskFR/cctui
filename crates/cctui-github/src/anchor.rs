//! GH-VIEW-2: comment anchoring — map a rendered-diff selection to a GitHub
//! review-comment anchor (`path`/`side`/`line`/`start_line`/`start_side`/
//! `commit_id`).
//!
//! This is the load-bearing correctness detail of the diff viewer (docs §6.2,
//! §11): a draft comment must land on the *exact* line it was authored against
//! when it is published (GH-VIEW-5). The draft UI (GH-VIEW-4) records a
//! [`DiffSelection`]; [`resolve`] turns it into a [`CommentAnchor`] by checking
//! it against the [`PullDiff`] it targets, or rejects it with an [`AnchorError`]
//! the UI can surface (rather than silently publishing onto the wrong line).
//!
//! The mapping rules (from GitHub's review-comment semantics):
//! - **Side.** `New` ⇒ `RIGHT` (head/new side), `Old` ⇒ `LEFT` (base/old side).
//! - **Line.** GitHub anchors on the side-local line number: a `New` selection
//!   uses the line's `new_line`, an `Old` selection its `old_line`. Only lines
//!   that actually appear in a hunk are anchorable — a line outside every hunk
//!   (e.g. far context the diff never rendered) is [`AnchorError::LineNotInDiff`].
//! - **Diffable side per kind.** A `del` line exists only on `LEFT`; an `add`
//!   line only on `RIGHT`; a `context` line on both. So a selection is anchorable
//!   on `side` iff some hunk line carries a matching side-local number for that
//!   side.
//! - **Path.** GitHub anchors comments on the **head-side path**. For a renamed
//!   file the selection's `path` is the new name; the old path is irrelevant to
//!   the anchor (we still match the diff file by its head `path`).
//! - **Force-push.** The selection carries the `head_sha` it was made against; if
//!   the diff has rotated to a new head SHA the selection is stale
//!   ([`AnchorError::StaleHeadSha`]) — its line numbers no longer refer to the
//!   same content.
//! - **Multi-line.** `start_line..=line` (inclusive) on a single side; both
//!   endpoints must be diffable on that side and `start_line <= line`.

use cctui_proto::github::{
    AnchorError, CommentAnchor, DiffFile, DiffLineKind, DiffSelection, DiffSide, PullDiff,
};

/// Resolve a reviewer's [`DiffSelection`] against the [`PullDiff`] it targets
/// into a GitHub [`CommentAnchor`], or explain why it cannot be anchored.
///
/// Pure over its inputs — no I/O, no DB — so it is exhaustively unit-testable
/// (see the tests below covering multi-hunk, renamed, and force-pushed PRs).
pub fn resolve(diff: &PullDiff, sel: &DiffSelection) -> Result<CommentAnchor, AnchorError> {
    // Force-push guard: a selection keyed to a stale head SHA refers to content
    // the diff no longer represents (docs §11). Detect before any line lookup.
    if diff.head_sha != sel.head_sha {
        return Err(AnchorError::StaleHeadSha {
            selection_sha: sel.head_sha.clone(),
            diff_sha: diff.head_sha.clone(),
        });
    }

    let file = diff
        .files
        .iter()
        .find(|f| f.path == sel.path)
        .ok_or(AnchorError::FileNotFound)?;

    // Validate the range endpoints (if any) and the end line, all on `sel.side`.
    if let Some(start) = sel.start_line {
        if start > sel.line {
            return Err(AnchorError::InvalidRange);
        }
        // Both endpoints must be diffable on the same side.
        if !line_in_diff(file, sel.side, start) {
            return Err(AnchorError::InvalidRange);
        }
    }

    if !line_in_diff(file, sel.side, sel.line) {
        return Err(AnchorError::LineNotInDiff);
    }

    Ok(CommentAnchor {
        path: file.path.clone(),
        commit_id: diff.head_sha.clone(),
        line: sel.line,
        side: sel.side,
        start_line: sel.start_line,
        // GitHub requires start_side alongside start_line; cctui never anchors a
        // range across columns, so it always matches `side`.
        start_side: sel.start_line.map(|_| sel.side),
    })
}

/// Whether `line` (a side-local number) appears on `side` in any hunk of the
/// file. A `New` side matches a line whose `new_line == line` (i.e. an `add` or
/// `context` line); an `Old` side matches `old_line == line` (a `del` or
/// `context` line). A line not present in any hunk is un-anchorable.
fn line_in_diff(file: &DiffFile, side: DiffSide, line: u32) -> bool {
    file.hunks.iter().flat_map(|h| &h.lines).any(|l| {
        // A context line carries both numbers and is anchorable on either side;
        // an add only on New, a del only on Old — exactly the side-local number.
        match side {
            DiffSide::New => {
                matches!(l.kind, DiffLineKind::Add | DiffLineKind::Context)
                    && l.new_line == Some(line)
            }
            DiffSide::Old => {
                matches!(l.kind, DiffLineKind::Del | DiffLineKind::Context)
                    && l.old_line == Some(line)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cctui_proto::github::{DiffHunk, DiffLine};

    fn line(kind: DiffLineKind, old: Option<u32>, new: Option<u32>) -> DiffLine {
        DiffLine { kind, content: "x".into(), old_line: old, new_line: new }
    }

    fn ctx(o: u32, n: u32) -> DiffLine {
        line(DiffLineKind::Context, Some(o), Some(n))
    }
    fn add(n: u32) -> DiffLine {
        line(DiffLineKind::Add, None, Some(n))
    }
    fn del(o: u32) -> DiffLine {
        line(DiffLineKind::Del, Some(o), None)
    }

    fn file(path: &str, prev: Option<&str>, status: &str, hunks: Vec<DiffHunk>) -> DiffFile {
        DiffFile {
            path: path.into(),
            previous_path: prev.map(str::to_string),
            status: status.into(),
            additions: 0,
            deletions: 0,
            hunks,
            truncated: false,
            binary: false,
            blob_sha: None,
        }
    }

    fn diff(head_sha: &str, files: Vec<DiffFile>) -> PullDiff {
        PullDiff {
            repo: "o/r".into(),
            number: 1,
            head_sha: head_sha.into(),
            total_files: files.len() as u32,
            total_changes: 0,
            huge: false,
            files,
        }
    }

    fn sel(path: &str, side: DiffSide, line: u32, start: Option<u32>, sha: &str) -> DiffSelection {
        DiffSelection {
            path: path.into(),
            side,
            line,
            start_line: start,
            head_sha: sha.into(),
        }
    }

    // A single-hunk file: ctx@1, del@2(old), add@2(new), ctx@3/3.
    fn single_hunk_file() -> DiffFile {
        let h = DiffHunk {
            old_start: 1,
            old_lines: 3,
            new_start: 1,
            new_lines: 3,
            header: None,
            lines: vec![ctx(1, 1), del(2), add(2), ctx(3, 3)],
        };
        file("a.rs", None, "modified", vec![h])
    }

    #[test]
    fn anchors_an_added_line_on_the_right() {
        let d = diff("sha1", vec![single_hunk_file()]);
        let a = resolve(&d, &sel("a.rs", DiffSide::New, 2, None, "sha1")).unwrap();
        assert_eq!(a.path, "a.rs");
        assert_eq!(a.commit_id, "sha1");
        assert_eq!(a.line, 2);
        assert_eq!(a.side, DiffSide::New);
        assert_eq!(a.side.github_token(), "RIGHT");
        assert_eq!(a.start_line, None);
        assert_eq!(a.start_side, None);
    }

    #[test]
    fn anchors_a_deleted_line_on_the_left() {
        let d = diff("sha1", vec![single_hunk_file()]);
        let a = resolve(&d, &sel("a.rs", DiffSide::Old, 2, None, "sha1")).unwrap();
        assert_eq!(a.side, DiffSide::Old);
        assert_eq!(a.side.github_token(), "LEFT");
        assert_eq!(a.line, 2);
    }

    #[test]
    fn context_line_anchorable_on_both_sides() {
        let d = diff("sha1", vec![single_hunk_file()]);
        // ctx(3,3): old-side 3 on LEFT and new-side 3 on RIGHT both resolve.
        assert!(resolve(&d, &sel("a.rs", DiffSide::Old, 3, None, "sha1")).is_ok());
        assert!(resolve(&d, &sel("a.rs", DiffSide::New, 3, None, "sha1")).is_ok());
    }

    #[test]
    fn add_line_not_anchorable_on_left() {
        let d = diff("sha1", vec![single_hunk_file()]);
        // The add line has no old-side number, so LEFT@2 is the del — fine —
        // but a new-only line number on the wrong side must not anchor: pick a
        // new-side-only number (the add's new_line=2 has no old equivalent at 99).
        let err = resolve(&d, &sel("a.rs", DiffSide::Old, 99, None, "sha1")).unwrap_err();
        assert_eq!(err, AnchorError::LineNotInDiff);
    }

    #[test]
    fn line_outside_any_hunk_is_unanchorable() {
        let d = diff("sha1", vec![single_hunk_file()]);
        let err = resolve(&d, &sel("a.rs", DiffSide::New, 500, None, "sha1")).unwrap_err();
        assert_eq!(err, AnchorError::LineNotInDiff);
    }

    // ---- §11 edge case: multi-hunk file -----------------------------------
    fn multi_hunk_file() -> DiffFile {
        // Hunk 1 around line 10; hunk 2 around line 100 — the gap between them
        // is un-anchorable (the classic "comment lands in the wrong hunk" bug).
        let h1 = DiffHunk {
            old_start: 10,
            old_lines: 2,
            new_start: 10,
            new_lines: 3,
            header: None,
            lines: vec![ctx(10, 10), add(11), ctx(11, 12)],
        };
        let h2 = DiffHunk {
            old_start: 100,
            old_lines: 3,
            new_start: 101,
            new_lines: 2,
            header: None,
            lines: vec![ctx(100, 101), del(101), del(102)],
        };
        file("m.rs", None, "modified", vec![h1, h2])
    }

    #[test]
    fn multi_hunk_anchors_in_the_correct_hunk() {
        let d = diff("sha1", vec![multi_hunk_file()]);
        // New-side 11 is the add in hunk 1.
        assert_eq!(resolve(&d, &sel("m.rs", DiffSide::New, 11, None, "sha1")).unwrap().line, 11);
        // Old-side 101/102 are dels in hunk 2.
        assert!(resolve(&d, &sel("m.rs", DiffSide::Old, 102, None, "sha1")).is_ok());
    }

    #[test]
    fn multi_hunk_gap_between_hunks_is_unanchorable() {
        let d = diff("sha1", vec![multi_hunk_file()]);
        // New-side 50 is between the two hunks — present in neither.
        let err = resolve(&d, &sel("m.rs", DiffSide::New, 50, None, "sha1")).unwrap_err();
        assert_eq!(err, AnchorError::LineNotInDiff);
    }

    // ---- §11 edge case: renamed file --------------------------------------
    #[test]
    fn renamed_file_anchors_on_head_path_not_old_path() {
        let h = DiffHunk {
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 2,
            header: None,
            lines: vec![ctx(1, 1), add(2)],
        };
        let f = file("new/name.rs", Some("old/name.rs"), "renamed", vec![h]);
        let d = diff("sha1", vec![f]);

        // Anchoring on the NEW path works and reports the new path.
        let a = resolve(&d, &sel("new/name.rs", DiffSide::New, 2, None, "sha1")).unwrap();
        assert_eq!(a.path, "new/name.rs");

        // Anchoring on the OLD path must NOT match (GitHub anchors on head path).
        let err = resolve(&d, &sel("old/name.rs", DiffSide::New, 2, None, "sha1")).unwrap_err();
        assert_eq!(err, AnchorError::FileNotFound);
    }

    // ---- §11 edge case: force-pushed PR (head SHA rotated) -----------------
    #[test]
    fn force_push_invalidates_a_stale_selection() {
        let d = diff("sha-new", vec![single_hunk_file()]);
        // Selection was made against the OLD head SHA → stale, detectable.
        let err = resolve(&d, &sel("a.rs", DiffSide::New, 2, None, "sha-old")).unwrap_err();
        assert_eq!(
            err,
            AnchorError::StaleHeadSha {
                selection_sha: "sha-old".into(),
                diff_sha: "sha-new".into(),
            }
        );
    }

    #[test]
    fn matching_head_sha_after_a_no_op_repush_still_anchors() {
        // Same content, same SHA: a re-fetch does not invalidate the draft.
        let d = diff("sha1", vec![single_hunk_file()]);
        assert!(resolve(&d, &sel("a.rs", DiffSide::New, 2, None, "sha1")).is_ok());
    }

    // ---- multi-line (start_line..=line) ranges -----------------------------
    #[test]
    fn anchors_a_multi_line_range_on_one_side() {
        let d = diff("sha1", vec![multi_hunk_file()]);
        // Range 101..=102 on the OLD side (two dels in hunk 2).
        let a = resolve(&d, &sel("m.rs", DiffSide::Old, 102, Some(101), "sha1")).unwrap();
        assert_eq!(a.line, 102);
        assert_eq!(a.start_line, Some(101));
        assert_eq!(a.start_side, Some(DiffSide::Old));
        assert_eq!(a.side, DiffSide::Old);
    }

    #[test]
    fn range_with_start_greater_than_end_is_invalid() {
        let d = diff("sha1", vec![multi_hunk_file()]);
        let err = resolve(&d, &sel("m.rs", DiffSide::Old, 101, Some(102), "sha1")).unwrap_err();
        assert_eq!(err, AnchorError::InvalidRange);
    }

    #[test]
    fn range_with_undiffable_start_is_invalid() {
        let d = diff("sha1", vec![multi_hunk_file()]);
        // End line 102 (del) is diffable, but start 50 is in the gap → invalid.
        let err = resolve(&d, &sel("m.rs", DiffSide::Old, 102, Some(50), "sha1")).unwrap_err();
        assert_eq!(err, AnchorError::InvalidRange);
    }

    #[test]
    fn single_line_when_start_equals_end_keeps_the_range() {
        let d = diff("sha1", vec![single_hunk_file()]);
        // start == line is a valid (degenerate) range; GitHub accepts it.
        let a = resolve(&d, &sel("a.rs", DiffSide::New, 2, Some(2), "sha1")).unwrap();
        assert_eq!(a.start_line, Some(2));
        assert_eq!(a.line, 2);
    }
}
