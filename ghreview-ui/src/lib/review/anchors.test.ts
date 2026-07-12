import { describe, expect, test } from "vitest";
import type { GithubFile } from "../api/types";
import { buildDiffModel } from "../diff/parse";
import type { ReviewDraftComment, ReviewThreadComment } from "../api/types";
import { buildAnchors, buildRowLocator, locateRow, rangeToAddress, rowToAddress } from "./anchors";

const file: GithubFile = {
  filename: "src/app.ts",
  status: "modified",
  additions: 2,
  deletions: 1,
  changes: 3,
  patch: ["@@ -1,3 +1,4 @@", " ctx", "-gone", "+added1", "+added2"].join("\n"),
};

const model = buildDiffModel([file]);

function draft(over: Partial<ReviewDraftComment>): ReviewDraftComment {
  return {
    id: "1",
    path: "src/app.ts",
    side: "RIGHT",
    line: 2,
    start_line: null,
    start_side: null,
    body: "x",
    created_at: null,
    updated_at: null,
    ...over,
  };
}

describe("anchors", () => {
  test("locator maps added lines to RIGHT and deleted to LEFT", () => {
    const loc = buildRowLocator(model);
    expect(locateRow(loc, { path: "src/app.ts", side: "RIGHT", line: 2 })).toBeDefined();
    expect(locateRow(loc, { path: "src/app.ts", side: "LEFT", line: 2 })).toBeDefined();
    expect(locateRow(loc, { path: "nope.ts", side: "RIGHT", line: 2 })).toBeUndefined();
  });

  test("rowToAddress reads add/del/context rows", () => {
    const addRow = model.rows.findIndex((r) => r.kind === "add");
    const addr = rowToAddress(model, addRow);
    expect(addr).toEqual({ path: "src/app.ts", side: "RIGHT", line: 2 });
    const fileRow = model.rows.findIndex((r) => r.kind === "file");
    expect(rowToAddress(model, fileRow)).toBeNull();
  });

  test("rangeToAddress attaches start_line for a multi-line same-side range", () => {
    const first = model.rows.findIndex((r) => r.kind === "add");
    const addr = rangeToAddress(model, first, first + 1);
    expect(addr?.side).toBe("RIGHT");
    expect(addr?.line).toBe(3);
    expect(addr?.start_line).toBe(2);
  });

  test("buildAnchors groups draft + published comments onto the right rows", () => {
    const published: ReviewThreadComment[] = [
      {
        id: 5,
        path: "src/app.ts",
        line: 2,
        original_line: null,
        side: "RIGHT",
        start_line: null,
        diff_hunk: null,
        body: "old",
        user: "bob",
        in_reply_to_id: null,
        created_at: null,
        html_url: null,
      },
    ];
    const anchors = buildAnchors(model, [draft({ line: 2 })], published);
    expect(anchors.length).toBe(1);
    expect(anchors[0]?.drafts.length).toBe(1);
    expect(anchors[0]?.published.length).toBe(1);
  });
});
