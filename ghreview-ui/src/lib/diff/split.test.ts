import { describe, expect, it } from "vitest";
import type { GithubFile } from "../api/types";
import { buildDiffModel } from "./parse";
import { buildSplitModel, type SplitRow } from "./split";

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

function codeText(cell: { row: { content: string } } | null): string | null {
  return cell ? cell.row.content : null;
}

describe("buildSplitModel", () => {
  it("pairs a context line with itself on both sides", () => {
    const model = buildDiffModel([file("@@ -1,1 +1,1 @@\n ctx")]);
    const split = buildSplitModel(model);
    const pair = split.rows.find((r) => r.kind === "pair") as Extract<SplitRow, { kind: "pair" }>;
    expect(pair.left).toBe(pair.right);
    expect(codeText(pair.left)).toBe("ctx");
  });

  it("zips removals against additions in a change block", () => {
    const model = buildDiffModel([file("@@ -1,2 +1,2 @@\n-old1\n-old2\n+new1\n+new2")]);
    const split = buildSplitModel(model);
    const pairs = split.rows.filter(
      (r): r is Extract<SplitRow, { kind: "pair" }> => r.kind === "pair",
    );
    expect(pairs).toHaveLength(2);
    expect(codeText(pairs[0].left)).toBe("old1");
    expect(codeText(pairs[0].right)).toBe("new1");
    expect(codeText(pairs[1].left)).toBe("old2");
    expect(codeText(pairs[1].right)).toBe("new2");
  });

  it("leaves a filler cell when one side is longer", () => {
    const model = buildDiffModel([file("@@ -1,1 +1,2 @@\n-old1\n+new1\n+new2")]);
    const split = buildSplitModel(model);
    const pairs = split.rows.filter(
      (r): r is Extract<SplitRow, { kind: "pair" }> => r.kind === "pair",
    );
    expect(pairs).toHaveLength(2);
    expect(codeText(pairs[1].left)).toBeNull();
    expect(codeText(pairs[1].right)).toBe("new2");
  });

  it("maps every unified code row to a split row", () => {
    const model = buildDiffModel([file("@@ -1,2 +1,2 @@\n ctx\n-old\n+new")]);
    const split = buildSplitModel(model);
    model.rows.forEach((row, i) => {
      if (row.kind === "add" || row.kind === "del" || row.kind === "context") {
        expect(split.unifiedToSplit.has(i)).toBe(true);
      }
    });
  });
});
