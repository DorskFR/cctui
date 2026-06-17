/**
 * The single-surface row model for the virtualized diff viewer (GH-VIEW-3).
 *
 * A `PullDiff` is a tree (files → hunks → lines); a virtualizer wants ONE flat
 * list it can index by offset. `flattenDiff` collapses the whole change set into
 * a single `DiffRow[]` — file headers, hunk headers, code lines, and
 * "collapsed unchanged region" markers — across EVERY file, so one
 * `@tanstack/svelte-virtual` instance renders only the on-screen slice of the
 * entire PR (scaling to thousands of files / 100k+ lines without putting them
 * all in the DOM).
 *
 * Two behaviours the doc (§6.2) calls for live here, as pure data transforms:
 *  - **importance ordering** — files are ordered source-before-support and by
 *    change size, not alphabetically (`orderFiles`);
 *  - **collapse unchanged regions** — long runs of context lines inside a hunk
 *    become a single collapsible marker row with lazy-expand (`COLLAPSE_*`).
 */
import type { PullDiff } from "@bindings/PullDiff";
import type { DiffFile } from "@bindings/DiffFile";
import type { DiffLine } from "@bindings/DiffLine";

/** A unique key for a file within the diff (its head-side path is unique). */
export type FileKey = string;

export type DiffRow =
  | {
      kind: "file";
      fileKey: FileKey;
      file: DiffFile;
      /** Index of this file among the ordered files, for "next/prev file" nav. */
      fileIndex: number;
    }
  | {
      kind: "hunk";
      fileKey: FileKey;
      header: string;
      /** Index of this hunk within its file, for "next/prev hunk" nav. */
      hunkIndex: number;
    }
  | {
      kind: "line";
      fileKey: FileKey;
      line: DiffLine;
    }
  | {
      kind: "collapsed";
      fileKey: FileKey;
      /** Stable id so expanding one region doesn't disturb others. */
      regionId: string;
      /** How many context lines this marker hides. */
      count: number;
    }
  | {
      kind: "binary" | "truncated" | "empty";
      fileKey: FileKey;
      file: DiffFile;
    };

/** Runs of unchanged context longer than this (away from a change) collapse into
 * a single lazy-expand marker; the edge lines stay visible for orientation. */
const COLLAPSE_THRESHOLD = 10;
/** Context lines kept visible on each side of a collapsed region. */
const COLLAPSE_CONTEXT = 3;

/**
 * Importance ordering (§6.2): source files before supporting files, then by
 * change size (larger diffs first), then by path for stability. "Supporting"
 * files are tests, lockfiles, generated bindings, snapshots, docs, and config —
 * the things a reviewer scans last. Deterministic and dependency-free (the
 * doc's "by reference count" is a future refinement; change size is the cheap,
 * checkout-free proxy we have from the diff alone).
 */
const SUPPORT_RE =
  /(^|\/)(tests?|__tests__|__snapshots__|fixtures?|e2e|spec|bindings|generated|vendor|node_modules|dist|build)(\/|$)|\.(lock|snap|min\.js|map)$|(^|\/)(package-lock\.json|yarn\.lock|pnpm-lock\.yaml|cargo\.lock|go\.sum)$|\.(test|spec)\.[a-z]+$|\.(md|markdown|txt|rst)$/i;

function supportRank(file: DiffFile): number {
  return SUPPORT_RE.test(file.path) ? 1 : 0;
}

export function orderFiles(files: DiffFile[]): DiffFile[] {
  return [...files].sort((a, b) => {
    const r = supportRank(a) - supportRank(b);
    if (r !== 0) return r;
    const sizeA = a.additions + a.deletions;
    const sizeB = b.additions + b.deletions;
    if (sizeA !== sizeB) return sizeB - sizeA;
    return a.path.localeCompare(b.path);
  });
}

/**
 * Flatten the (importance-ordered) diff into the single virtualized surface.
 *
 * `expanded` is the set of collapsed-region ids the user has lazy-expanded;
 * `collapsedFiles` is the set of file keys whose body is folded away (only the
 * file header row shows). Both are passed in so the caller owns the UI state and
 * a re-flatten is a cheap pure recompute.
 */
export function flattenDiff(
  diff: PullDiff,
  expanded: Set<string>,
  collapsedFiles: Set<string>,
): DiffRow[] {
  const rows: DiffRow[] = [];
  const ordered = orderFiles(diff.files);

  ordered.forEach((file, fileIndex) => {
    const fileKey = file.path;
    rows.push({ kind: "file", file, fileKey, fileIndex });
    if (collapsedFiles.has(fileKey)) return;

    if (file.binary) {
      rows.push({ kind: "binary", file, fileKey });
      return;
    }
    if (file.truncated) {
      rows.push({ kind: "truncated", file, fileKey });
      return;
    }
    if (file.hunks.length === 0) {
      rows.push({ kind: "empty", file, fileKey });
      return;
    }

    file.hunks.forEach((hunk, hunkIndex) => {
      rows.push({
        kind: "hunk",
        fileKey,
        header: hunk.header ?? formatHunkHeader(hunk),
        hunkIndex,
      });
      appendHunkLines(rows, fileKey, hunkIndex, hunk.lines, expanded);
    });
  });

  return rows;
}

/** Emit a hunk's lines, folding long interior runs of context into a single
 * collapsible marker (unless the user already expanded that region). */
function appendHunkLines(
  rows: DiffRow[],
  fileKey: FileKey,
  hunkIndex: number,
  lines: DiffLine[],
  expanded: Set<string>,
): void {
  let i = 0;
  let regionN = 0;
  while (i < lines.length) {
    if (lines[i].kind !== "context") {
      rows.push({ kind: "line", fileKey, line: lines[i] });
      i++;
      continue;
    }
    // Gather the maximal run of context lines starting at i.
    let j = i;
    while (j < lines.length && lines[j].kind === "context") j++;
    const run = lines.slice(i, j);
    const atStart = i === 0;
    const atEnd = j === lines.length;
    // Keep edge context for orientation; only the interior collapses.
    const head = atStart ? 0 : COLLAPSE_CONTEXT;
    const tail = atEnd ? 0 : COLLAPSE_CONTEXT;
    const hidden = run.length - head - tail;
    const regionId = `${fileKey}#${hunkIndex}.${regionN++}`;

    if (run.length <= COLLAPSE_THRESHOLD || expanded.has(regionId)) {
      for (const l of run) rows.push({ kind: "line", fileKey, line: l });
    } else {
      for (let k = 0; k < head; k++)
        rows.push({ kind: "line", fileKey, line: run[k] });
      rows.push({ kind: "collapsed", fileKey, regionId, count: hidden });
      for (let k = run.length - tail; k < run.length; k++)
        rows.push({ kind: "line", fileKey, line: run[k] });
    }
    i = j;
  }
}

function formatHunkHeader(hunk: {
  old_start: number;
  old_lines: number;
  new_start: number;
  new_lines: number;
}): string {
  return `@@ -${hunk.old_start},${hunk.old_lines} +${hunk.new_start},${hunk.new_lines} @@`;
}

/** Row indices of each file/hunk header, for keyboard next/prev navigation. */
export function navIndex(rows: DiffRow[]): {
  files: number[];
  hunks: number[];
} {
  const files: number[] = [];
  const hunks: number[] = [];
  rows.forEach((r, i) => {
    if (r.kind === "file") files.push(i);
    else if (r.kind === "hunk") hunks.push(i);
  });
  return { files, hunks };
}
