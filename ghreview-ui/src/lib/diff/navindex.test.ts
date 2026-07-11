import { describe, expect, it } from "vitest";
import type { GithubFile } from "../api/types";
import { buildNavIndex, nextFile, nextHunk, prevFile, prevHunk } from "./navindex";
import { buildDiffModel } from "./parse";

function mk(patch: string, filename: string): GithubFile {
  return { filename, status: "modified", additions: 0, deletions: 0, changes: 0, patch };
}

const patchA = ["@@ -1,1 +1,1 @@", "-a", "+b", "@@ -9,1 +9,1 @@", "-c", "+d"].join("\n");
const patchB = ["@@ -1,1 +1,1 @@", "-e", "+f"].join("\n");
const model = buildDiffModel([mk(patchA, "a.ts"), mk(patchB, "b.ts")]);
const nav = buildNavIndex(model);

describe("buildNavIndex", () => {
  it("indexes every file and hunk", () => {
    expect(nav.files).toHaveLength(2);
    expect(nav.hunks).toHaveLength(3);
    expect(nav.files.map((f) => f.rowIndex)).toEqual(
      model.files.map((f) => f.fileRowIndex),
    );
  });

  it("hunk targets are strictly increasing", () => {
    const rows = nav.hunks.map((h) => h.rowIndex);
    expect([...rows].sort((a, b) => a - b)).toEqual(rows);
  });
});

describe("navigation stepping", () => {
  it("nextFile / prevFile move between file headers", () => {
    const first = nav.files[0].rowIndex;
    const second = nav.files[1].rowIndex;
    expect(nextFile(nav, first)?.rowIndex).toBe(second);
    expect(prevFile(nav, second)?.rowIndex).toBe(first);
    expect(nextFile(nav, second)).toBeNull();
    expect(prevFile(nav, first)).toBeNull();
  });

  it("nextHunk / prevHunk walk hunks across files", () => {
    let row = -1;
    const seen: number[] = [];
    let t = nextHunk(nav, row);
    while (t) {
      seen.push(t.rowIndex);
      row = t.rowIndex;
      t = nextHunk(nav, row);
    }
    expect(seen).toHaveLength(3);
    expect(prevHunk(nav, seen[2])?.rowIndex).toBe(seen[1]);
  });
});
