import { describe, expect, it } from "vitest";
import type { GithubFile } from "../api/types";
import { buildDiffModel, parseHunkHeader } from "./parse";

function file(patch: string, overrides: Partial<GithubFile> = {}): GithubFile {
  return {
    filename: "src/a.ts",
    status: "modified",
    additions: 0,
    deletions: 0,
    changes: 0,
    patch,
    ...overrides,
  };
}

describe("parseHunkHeader", () => {
  it("parses counts and section", () => {
    const h = parseHunkHeader("@@ -3,7 +3,9 @@ function foo() {");
    expect(h).toEqual({ oldStart: 3, oldCount: 7, newStart: 3, newCount: 9, section: "function foo() {" });
  });

  it("defaults omitted counts to 1", () => {
    const h = parseHunkHeader("@@ -1 +1 @@");
    expect(h).toMatchObject({ oldCount: 1, newCount: 1 });
  });

  it("returns null for non-headers", () => {
    expect(parseHunkHeader("+ added")).toBeNull();
  });
});

describe("buildDiffModel", () => {
  const patch = ["@@ -1,3 +1,4 @@", " ctx", "-gone", "+added", "+more", " tail"].join("\n");

  it("assigns correct old/new line numbers", () => {
    const model = buildDiffModel([file(patch)]);
    const rows = model.rows;
    expect(rows[0].kind).toBe("file");
    expect(rows[1].kind).toBe("hunk");

    const ctx = rows.find((r) => r.kind === "context" && r.content === "ctx");
    expect(ctx).toMatchObject({ oldLine: 1, newLine: 1 });

    const del = rows.find((r) => r.kind === "del");
    expect(del).toMatchObject({ oldLine: 2, newLine: null });

    const adds = rows.filter((r) => r.kind === "add");
    expect(adds.map((r) => r.newLine)).toEqual([2, 3]);

    const tail = rows.find((r) => r.content === "tail");
    expect(tail).toMatchObject({ oldLine: 3, newLine: 4 });
  });

  it("tracks file and hunk boundaries", () => {
    const model = buildDiffModel([file(patch)]);
    expect(model.files).toHaveLength(1);
    const f = model.files[0];
    expect(f.hunks).toHaveLength(1);
    expect(f.hunks[0].rowStart).toBe(1);
    expect(f.rowEnd).toBe(model.rows.length);
  });

  it("handles multiple hunks and files", () => {
    const twoHunks = [
      "@@ -1,1 +1,1 @@",
      "-a",
      "+b",
      "@@ -10,1 +10,2 @@",
      " keep",
      "+new",
    ].join("\n");
    const model = buildDiffModel([file(twoHunks), file("", { filename: "img.png" })]);
    expect(model.files[0].hunks).toHaveLength(2);
    expect(model.files[0].hunks[1].header.newStart).toBe(10);
    expect(model.files[1].binary).toBe(true);
    expect(model.files[1].hunks).toHaveLength(0);
  });

  it("ignores no-newline markers", () => {
    const p = ["@@ -1 +1 @@", "-a", "+b", "\\ No newline at end of file"].join("\n");
    const model = buildDiffModel([file(p)]);
    expect(model.rows.some((r) => r.content.includes("No newline"))).toBe(false);
  });
});
