import { describe, it, expect } from "vitest";
import {
  blockLabel,
  blockAskMessage,
  promoteAnswerToDraft,
  type BlockSelection,
} from "./ask";

const range: BlockSelection = {
  path: "src/foo.rs",
  side: "new",
  line: 42,
  startLine: 40,
  snippet: "fn foo() {\n    bar();\n}",
};
const single: BlockSelection = {
  path: "src/bar.rs",
  side: "old",
  line: 7,
  startLine: null,
  snippet: "let x = 1;",
};

describe("blockLabel", () => {
  it("renders a range as path:start-end", () => {
    expect(blockLabel(range)).toBe("src/foo.rs:40-42");
  });
  it("renders a single line as path:line", () => {
    expect(blockLabel(single)).toBe("src/bar.rs:7");
  });
  it("collapses startLine == line to a single line label", () => {
    expect(blockLabel({ ...single, startLine: 7 })).toBe("src/bar.rs:7");
  });
});

describe("blockAskMessage", () => {
  it("carries path, line range, side, and the snippet (no checkout)", () => {
    const msg = blockAskMessage(range);
    expect(msg).toContain("src/foo.rs:40-42");
    expect(msg).toContain("(head side)");
    // snippet is fenced verbatim so the agent gets block context inline
    expect(msg).toContain("```\nfn foo() {\n    bar();\n}\n```");
    // no file:// or checkout path is ever sent
    expect(msg).not.toMatch(/checkout|file:\/\//);
  });

  it("labels the base side for an old-side selection", () => {
    expect(blockAskMessage(single)).toContain("(base side)");
  });

  it("appends the reviewer's question when present", () => {
    const msg = blockAskMessage(range, "  Is bar() safe here?  ");
    expect(msg).toContain("Is bar() safe here?");
    // question is trimmed and goes after the snippet block
    expect(msg.indexOf("Is bar()")).toBeGreaterThan(msg.indexOf("```"));
  });

  it("omits an empty/whitespace question", () => {
    expect(blockAskMessage(range, "   ")).toBe(blockAskMessage(range));
  });
});

describe("promoteAnswerToDraft", () => {
  it("anchors a range answer to the same block (start_line kept)", () => {
    const draft = promoteAnswerToDraft(range, "  This looks fine.  ");
    expect(draft).toEqual({
      path: "src/foo.rs",
      side: "new",
      line: 42,
      start_line: 40,
      body: "This looks fine.",
      in_reply_to: null,
    });
  });

  it("drops start_line for a single-line selection", () => {
    const draft = promoteAnswerToDraft(single, "nit");
    expect(draft.start_line).toBeNull();
    expect(draft.line).toBe(7);
    expect(draft.side).toBe("old");
  });

  it("drops start_line when it equals the end line", () => {
    expect(promoteAnswerToDraft({ ...range, startLine: 42 }, "x").start_line).toBeNull();
  });
});
