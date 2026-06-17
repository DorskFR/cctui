/**
 * The block↔conversation bridge (GH-AGENT-3, docs §6.3).
 *
 * Two pure transforms wiring the diff viewer to a PR's review session:
 *
 *  1. **"Ask the agent about this block"** — turn a reviewer's diff selection
 *     (a `BlockSelection`: path + line range + the snippet text) into a chat
 *     message we inject into the linked review session via `ws.sendMessage`.
 *     Per steering (docs §6.3) the agent does NOT get a served checkout — it
 *     gets the snippet / file+line chunk AS message context, inline.
 *
 *  2. **"Promote answer to draft comment"** — take the agent's answer (a
 *     conversation message) and map it back onto the same block to produce the
 *     `CreateDraftComment` body the VIEW-4 draft store accepts, anchored to that
 *     block (VIEW-2 coordinates). Human curates; nothing reaches GitHub until
 *     Publish (VIEW-5).
 *
 * Both are pure so they are unit-tested without a socket or a DOM.
 */
import type { DiffSide } from "@bindings/DiffSide";
import type { CreateDraftComment } from "@bindings/CreateDraftComment";

/** A reviewer's block selection in the diff, ready to hand to the agent. The
 *  `line`/`startLine` are GH-VIEW-2 coordinates on `side`; `snippet` is the
 *  selected code text, sent inline so the agent needs no checkout. */
export interface BlockSelection {
  path: string;
  side: DiffSide;
  /** END line of the range (1-based, on `side`). */
  line: number;
  /** START line of a multi-line range; `null`/equal-to-`line` for one line. */
  startLine: number | null;
  /** The selected lines' text (no diff markers), newline-joined. */
  snippet: string;
}

/** Human-readable `path:start-end` (or `path:line` for a single line). */
export function blockLabel(sel: BlockSelection): string {
  const start = sel.startLine != null && sel.startLine < sel.line ? sel.startLine : null;
  return start != null ? `${sel.path}:${start}-${sel.line}` : `${sel.path}:${sel.line}`;
}

/**
 * Build the chat message injected into the review session for "Ask the agent
 * about this block". It carries the location (path + line range + side) and the
 * snippet text fenced as code, plus the reviewer's optional question — so the
 * agent has full block context with no checkout (docs §6.3).
 */
export function blockAskMessage(sel: BlockSelection, question?: string): string {
  const sideLabel = sel.side === "old" ? "base" : "head";
  const lines = [
    `About \`${blockLabel(sel)}\` (${sideLabel} side):`,
    "",
    "```",
    sel.snippet,
    "```",
  ];
  const q = question?.trim();
  if (q) {
    lines.push("", q);
  }
  return lines.join("\n");
}

/**
 * Map an agent answer + the block it answers about onto a `CreateDraftComment`
 * for the VIEW-4 draft store, anchored to that block (VIEW-2 coordinates). A
 * single-line selection drops `start_line`; a range keeps it. Always a fresh
 * top-level inline comment (no `in_reply_to`).
 */
export function promoteAnswerToDraft(
  sel: BlockSelection,
  answer: string,
): CreateDraftComment {
  const start =
    sel.startLine != null && sel.startLine < sel.line ? sel.startLine : null;
  return {
    path: sel.path,
    side: sel.side,
    line: sel.line,
    start_line: start,
    body: answer.trim(),
    in_reply_to: null,
  };
}
