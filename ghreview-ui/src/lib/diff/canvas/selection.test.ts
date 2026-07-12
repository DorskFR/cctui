import { describe, expect, it } from "vitest";
import type { GithubFile } from "../../api/types";
import { buildDiffModel } from "../parse";
import {
  normalizeSelection,
  rangeToClipboardText,
  selectionEvent,
  selectionRowIndexes,
} from "./selection";

const FILE: GithubFile = {
  filename: "src/a.ts",
  status: "modified",
  additions: 0,
  deletions: 0,
  changes: 0,
  patch: ["@@ -1,3 +1,3 @@", " a", "-b", "+c"].join("\n"),
};

describe("selection math", () => {
  it("normalizes regardless of drag direction", () => {
    expect(normalizeSelection({ anchor: 5, head: 2 })).toEqual({ start: 2, end: 5 });
    expect(normalizeSelection({ anchor: 2, head: 5 })).toEqual({ start: 2, end: 5 });
  });

  it("expands to an inclusive row index list", () => {
    expect(selectionRowIndexes({ anchor: 3, head: 1 })).toEqual([1, 2, 3]);
  });
});

describe("selection event emitted to the renderer seam", () => {
  it("carries the normalized range plus file/hunk of the first row", () => {
    const model = buildDiffModel([FILE]);
    const ev = selectionEvent(model, { anchor: 3, head: 1 });
    expect(ev.start).toBe(1);
    expect(ev.end).toBe(3);
    expect(ev.rows).toHaveLength(3);
    expect(ev.fileIndex).toBe(model.rows[1].fileIndex);
    expect(ev.hunkIndex).toBe(model.rows[1].hunkIndex);
  });
});

describe("range copy", () => {
  it("reconstructs unified-diff prefixes for the selected lines", () => {
    const model = buildDiffModel([FILE]);
    const text = rangeToClipboardText(model, { anchor: 2, head: 4 });
    expect(text).toBe(" a\n-b\n+c");
  });
});
