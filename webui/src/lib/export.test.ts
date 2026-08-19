import { describe, expect, it } from "vitest";
import type { AgentEvent } from "@bindings/AgentEvent";
import type { SessionListItem } from "@bindings/SessionListItem";
import {
  allFilter,
  defaultFilter,
} from "$lib/components/organisms/conversation/filters";
import type { MsgCategory } from "$lib/components/organisms/conversation/types";
import {
  buildConversationHtml,
  conversationToMarkdown,
  type ExportOpts,
} from "./export";

const opts = (
  overrides: Partial<Record<MsgCategory, boolean>> = {},
): ExportOpts => ({
  msgFilter: { ...allFilter(true), ...overrides },
  prettyJson: true,
  prettyDiff: true,
  prettyTables: true,
});

const only = (...cats: MsgCategory[]): ExportOpts => {
  const msgFilter = allFilter(false);
  for (const c of cats) msgFilter[c] = true;
  return { ...opts(), msgFilter };
};

const session = { id: "s1", name: "demo" } as SessionListItem;

const text = (
  content: string,
  ts: number,
  kind: string | null = null,
): AgentEvent => ({
  type: "text",
  content,
  meta: false,
  kind,
  ts,
  message_id: null,
  usage: null,
  seq: null,
});

const events: AgentEvent[] = [
  text("▷ User: launch-the-deploy", 1),
  text("▷ User: <system-reminder>stay-terse</system-reminder>", 2),
  text("weighing-the-options", 3, "thinking"),
  text("provider-withheld-this", 4, "redacted_thinking"),
  text("here-is-the-answer", 5),
  text("image-attachment-stub", 6, "attachment"),
  text("· permission mode: plan-only", 7, "system_marker"),
  {
    type: "tool_call",
    tool: "Bash",
    input: { command: "ls /tmp/tool-call-dir" },
    ts: 8,
    seq: null,
  },
  {
    type: "tool_call",
    tool: "mcp__pg__query",
    input: { sql: "select mcp_call" },
    ts: 9,
    seq: null,
  },
  {
    type: "tool_result",
    tool: "Bash",
    output_summary: "tool-result-payload",
    ts: 10,
    seq: null,
  },
  { type: "compact_summary", content: "compacted-history", ts: 11, seq: null },
  { type: "context_reset", ts: 12, seq: null },
  {
    type: "turn_summary",
    detail: "turn-wrapped-up",
    status_category: null,
    needs_action: false,
    ts: 13,
    seq: null,
  },
];

const NEEDLES: [MsgCategory, string][] = [
  ["user", "launch-the-deploy"],
  ["system", "stay-terse"],
  ["thinking", "weighing-the-options"],
  ["redacted", "provider-withheld-this"],
  ["assistant", "here-is-the-answer"],
  ["attachment", "image-attachment-stub"],
  ["marker", "plan-only"],
  ["tool", "tool-call-dir"],
  ["mcp", "select mcp_call"],
  ["result", "tool-result-payload"],
  ["compact", "compacted-history"],
  ["reset", "context reset"],
  ["summary", "turn-wrapped-up"],
];

describe("conversationToMarkdown", () => {
  it("includes every category by default", () => {
    const md = conversationToMarkdown(session, events, opts());
    for (const [, needle] of NEEDLES) expect(md).toContain(needle);
  });

  it.each(NEEDLES)("omits %s content when it is switched off", (cat, needle) => {
    const md = conversationToMarkdown(session, events, opts({ [cat]: false }));
    expect(md).not.toContain(needle);
  });

  it.each(NEEDLES)(
    "keeps only %s content when it is the sole category",
    (cat, needle) => {
      const md = conversationToMarkdown(session, events, only(cat));
      expect(md).toContain(needle);
      for (const [other, otherNeedle] of NEEDLES) {
        if (other !== cat) expect(md).not.toContain(otherNeedle);
      }
    },
  );

  it("exports nothing but the header with every category off", () => {
    const md = conversationToMarkdown(session, events, opts(allFilter(false)));
    for (const [, needle] of NEEDLES) expect(md).not.toContain(needle);
    expect(md).toContain("# demo");
  });
});

describe("buildConversationHtml", () => {
  it("honours the same filter as the markdown path", () => {
    const html = buildConversationHtml(session, events, opts());
    for (const [, needle] of NEEDLES) expect(html).toContain(needle);

    const narrow = buildConversationHtml(session, events, only("assistant"));
    expect(narrow).toContain("here-is-the-answer");
    expect(narrow).not.toContain("weighing-the-options");
    expect(narrow).not.toContain("plan-only");
  });

  it("still exports a full transcript under the shipped defaults", () => {
    const html = buildConversationHtml(session, events, {
      ...opts(),
      msgFilter: defaultFilter(),
    });
    expect(html).toContain("here-is-the-answer");
    expect(html).toContain("tool-result-payload");
  });
});
