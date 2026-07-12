import { describe, expect, it } from "vitest";
import type { GithubFile } from "../api/types";
import { collapseViewedFiles } from "./collapse";
import { buildDiffModel } from "./parse";
import { buildFileTree, collectFilePaths, isFullyViewed, viewedProgress } from "./tree";

function file(filename: string): GithubFile {
  return {
    filename,
    status: "modified",
    additions: 1,
    deletions: 0,
    changes: 1,
    patch: "@@ -1 +1 @@\n-old\n+new",
  };
}

function model(names: string[]) {
  return buildDiffModel(names.map(file));
}

describe("buildFileTree", () => {
  it("nests files under directories and compresses single-child chains", () => {
    const m = model(["src/lib/a.ts", "src/lib/b.ts", "README.md"]);
    const tree = buildFileTree(m.files);
    const names = tree.map((n) => n.name).sort();
    expect(names).toEqual(["README.md", "src/lib"]);
    const dir = tree.find((n) => n.type === "dir");
    expect(dir && collectFilePaths(dir).sort()).toEqual(["src/lib/a.ts", "src/lib/b.ts"]);
  });

  it("collectFilePaths gathers every file under a directory", () => {
    const m = model(["a/x.ts", "a/b/y.ts", "a/b/z.ts"]);
    const [root] = buildFileTree(m.files);
    expect(collectFilePaths(root).sort()).toEqual(["a/b/y.ts", "a/b/z.ts", "a/x.ts"]);
  });
});

describe("viewedProgress", () => {
  it("counts viewed files under a node", () => {
    const m = model(["a/x.ts", "a/y.ts", "a/z.ts"]);
    const [root] = buildFileTree(m.files);
    const viewed = new Set(["a/x.ts", "a/y.ts"]);
    expect(viewedProgress(root, viewed)).toEqual({ viewed: 2, total: 3 });
    expect(isFullyViewed(root, viewed)).toBe(false);
    expect(isFullyViewed(root, new Set(["a/x.ts", "a/y.ts", "a/z.ts"]))).toBe(true);
  });
});

describe("collapseViewedFiles", () => {
  it("hides body rows of viewed files, keeping a collapsed header", () => {
    const m = model(["a.ts", "b.ts"]);
    const rowsBefore = m.rows.length;
    const collapsed = collapseViewedFiles(m, { viewed: new Set(["a.ts"]) });
    expect(collapsed.rows.length).toBeLessThan(rowsBefore);

    const a = collapsed.files.find((f) => f.filename === "a.ts");
    const b = collapsed.files.find((f) => f.filename === "b.ts");
    expect(a?.collapsed).toBe(true);
    expect(a?.hiddenLines).toBeGreaterThan(0);
    expect(b?.collapsed).toBe(false);

    const aHeader = collapsed.rows[a?.fileRowIndex ?? -1];
    expect(aHeader.collapsed).toBe(true);
    expect(aHeader.content).toContain("lines hidden");
  });

  it("keeps b's rows navigable — row indices and hunks stay consistent", () => {
    const m = model(["a.ts", "b.ts"]);
    const collapsed = collapseViewedFiles(m, { viewed: new Set(["a.ts"]) });
    const b = collapsed.files.find((f) => f.filename === "b.ts");
    expect(b).toBeTruthy();
    if (!b) return;
    expect(collapsed.rows[b.fileRowIndex].kind).toBe("file");
    for (const hunk of b.hunks) {
      expect(collapsed.rows[hunk.rowStart].kind).toBe("hunk");
      expect(hunk.rowStart).toBeGreaterThanOrEqual(b.rowStart);
      expect(hunk.rowEnd).toBeLessThanOrEqual(b.rowEnd);
    }
  });

  it("an expanded viewed file is not collapsed", () => {
    const m = model(["a.ts"]);
    const collapsed = collapseViewedFiles(m, {
      viewed: new Set(["a.ts"]),
      expanded: new Set(["a.ts"]),
    });
    expect(collapsed.files[0].collapsed).toBe(false);
    expect(collapsed.rows.length).toBe(m.rows.length);
  });
});
