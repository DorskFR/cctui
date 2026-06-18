import { describe, it, expect } from "vitest";
import { orderFiles, flattenDiff, navIndex } from "./rows";
import type { PullDiff } from "@bindings/PullDiff";
import type { DiffFile } from "@bindings/DiffFile";
import type { DiffLine } from "@bindings/DiffLine";
import type { DiffHunk } from "@bindings/DiffHunk";

function file(path: string, partial: Partial<DiffFile> = {}): DiffFile {
  return {
    path,
    previous_path: null,
    status: "modified",
    additions: 0,
    deletions: 0,
    hunks: [],
    truncated: false,
    binary: false,
    blob_sha: null,
    ...partial,
  };
}

function ctx(n: number): DiffLine {
  return { kind: "context", content: `ctx ${n}`, old_line: n, new_line: n };
}
function add(n: number): DiffLine {
  return { kind: "add", content: `add ${n}`, old_line: null, new_line: n };
}
function del(n: number): DiffLine {
  return { kind: "del", content: `del ${n}`, old_line: n, new_line: null };
}

function hunk(lines: DiffLine[]): DiffHunk {
  return {
    old_start: 1,
    old_lines: lines.length,
    new_start: 1,
    new_lines: lines.length,
    header: null,
    lines,
  };
}

function diffOf(files: DiffFile[]): PullDiff {
  return {
    repo: "a/b",
    number: 1,
    head_sha: "sha",
    total_files: files.length,
    total_changes: 0,
    huge: false,
    files,
  };
}

describe("orderFiles (importance ordering)", () => {
  it("puts source files before support files, regardless of alphabetical order", () => {
    const ordered = orderFiles([
      file("package-lock.json", { additions: 999 }),
      file("src/app.ts", { additions: 1 }),
      file("README.md", { additions: 50 }),
      file("src/__tests__/app.test.ts", { additions: 80 }),
    ]);
    // Source (non-support) first, then support; src/app.ts must precede the
    // lockfile/test/docs even though it's the smallest change.
    expect(ordered[0].path).toBe("src/app.ts");
    const supportPaths = ordered.slice(1).map((f) => f.path);
    expect(supportPaths).toContain("package-lock.json");
    expect(supportPaths).toContain("src/__tests__/app.test.ts");
    expect(supportPaths).toContain("README.md");
  });

  it("orders by change size within the same rank (larger first)", () => {
    const ordered = orderFiles([
      file("src/small.ts", { additions: 2 }),
      file("src/big.ts", { additions: 100 }),
    ]);
    expect(ordered.map((f) => f.path)).toEqual(["src/big.ts", "src/small.ts"]);
  });
});

describe("flattenDiff (single surface + collapse)", () => {
  it("collapses a long interior run of context into one marker, keeping edges", () => {
    const lines = [
      add(1),
      ...Array.from({ length: 20 }, (_, i) => ctx(i + 2)),
      add(30),
    ];
    const d = diffOf([
      file("src/a.ts", {
        additions: 2,
        hunks: [hunk(lines)],
      }),
    ]);
    const rows = flattenDiff(d, new Set(), new Set());
    const collapsed = rows.filter((r) => r.kind === "collapsed");
    expect(collapsed).toHaveLength(1);
    // 20 context - 3 head - 3 tail = 14 hidden.
    expect(collapsed[0].kind === "collapsed" && collapsed[0].count).toBe(14);
    // The two change lines remain present.
    expect(
      rows.filter((r) => r.kind === "line" && r.line.kind === "add"),
    ).toHaveLength(2);
  });

  it("lazy-expands a region when its id is in the expanded set", () => {
    const lines = [
      add(1),
      ...Array.from({ length: 20 }, (_, i) => ctx(i + 2)),
      add(30),
    ];
    const d = diffOf([
      file("src/a.ts", {
        hunks: [hunk(lines)],
      }),
    ]);
    const collapsed = flattenDiff(d, new Set(), new Set()).find(
      (r) => r.kind === "collapsed",
    );
    const id = collapsed!.kind === "collapsed" ? collapsed!.regionId : "";
    const rows = flattenDiff(d, new Set([id]), new Set());
    expect(rows.some((r) => r.kind === "collapsed")).toBe(false);
    expect(
      rows.filter((r) => r.kind === "line" && r.line.kind === "context"),
    ).toHaveLength(20);
  });

  it("folds a file to just its header when collapsed", () => {
    const d = diffOf([
      file("src/a.ts", {
        hunks: [hunk([add(1)])],
      }),
    ]);
    const rows = flattenDiff(d, new Set(), new Set(["src/a.ts"]));
    expect(rows).toHaveLength(1);
    expect(rows[0].kind).toBe("file");
  });

  it("renders binary and truncated files as a single note row", () => {
    const d = diffOf([
      file("img.png", { binary: true }),
      file("huge.txt", { truncated: true }),
    ]);
    const rows = flattenDiff(d, new Set(), new Set());
    expect(rows.some((r) => r.kind === "binary")).toBe(true);
    expect(rows.some((r) => r.kind === "truncated")).toBe(true);
  });
});

describe("flattenDiff (split / side-by-side mode)", () => {
  it("pairs context lines onto both sides", () => {
    const d = diffOf([file("src/a.ts", { hunks: [hunk([ctx(1), ctx(2)])] })]);
    const rows = flattenDiff(d, new Set(), new Set(), "split");
    const pairs = rows.filter((r) => r.kind === "pair");
    expect(pairs).toHaveLength(2);
    // A context pair carries the same line object on both sides.
    expect(
      pairs.every(
        (p) => p.kind === "pair" && p.left === p.right && p.left !== null,
      ),
    ).toBe(true);
    // No unified `line` rows in split mode.
    expect(rows.some((r) => r.kind === "line")).toBe(false);
  });

  it("zips a balanced change block: del left, add right, same row", () => {
    const d = diffOf([
      file("src/a.ts", { hunks: [hunk([del(1), del(2), add(1), add(2)])] }),
    ]);
    const pairs = flattenDiff(d, new Set(), new Set(), "split").filter(
      (r) => r.kind === "pair",
    );
    expect(pairs).toHaveLength(2);
    expect(pairs[0].kind === "pair" && pairs[0].left?.kind).toBe("del");
    expect(pairs[0].kind === "pair" && pairs[0].right?.kind).toBe("add");
    expect(pairs[1].kind === "pair" && pairs[1].left?.kind).toBe("del");
    expect(pairs[1].kind === "pair" && pairs[1].right?.kind).toBe("add");
  });

  it("leaves the surplus side null for an uneven change block", () => {
    // 1 removal, 3 additions → row0 del|add, rows 1-2 null|add.
    const d = diffOf([
      file("src/a.ts", { hunks: [hunk([del(1), add(1), add(2), add(3)])] }),
    ]);
    const pairs = flattenDiff(d, new Set(), new Set(), "split").filter(
      (r) => r.kind === "pair",
    );
    expect(pairs).toHaveLength(3);
    expect(pairs[0].kind === "pair" && pairs[0].left?.kind).toBe("del");
    expect(pairs[1].kind === "pair" && pairs[1].left).toBeNull();
    expect(pairs[2].kind === "pair" && pairs[2].left).toBeNull();
    expect(pairs.every((p) => p.kind === "pair" && p.right?.kind === "add")).toBe(
      true,
    );
  });

  it("still collapses long context runs in split mode", () => {
    const lines = [
      add(1),
      ...Array.from({ length: 20 }, (_, i) => ctx(i + 2)),
      add(30),
    ];
    const d = diffOf([file("src/a.ts", { hunks: [hunk(lines)] })]);
    const rows = flattenDiff(d, new Set(), new Set(), "split");
    expect(rows.filter((r) => r.kind === "collapsed")).toHaveLength(1);
  });
});

describe("navIndex", () => {
  it("records file and hunk header row positions", () => {
    const d = diffOf([
      file("src/a.ts", {
        hunks: [hunk([add(1)])],
      }),
    ]);
    const rows = flattenDiff(d, new Set(), new Set());
    const nav = navIndex(rows);
    expect(nav.files).toHaveLength(1);
    expect(nav.hunks).toHaveLength(1);
    expect(rows[nav.files[0]].kind).toBe("file");
    expect(rows[nav.hunks[0]].kind).toBe("hunk");
  });
});
