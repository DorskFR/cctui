// Shared types + constants for the conversation drawer and its sub-components,
// extracted from ConversationDrawer.svelte (no behavior change).
import type { TokenUsage as TokenUsageT } from '@bindings/TokenUsage';

// ── Message-type tag filter (CCT-250 item 2) ──────────────────────────────
// Each message type is a clickable badge with include/exclude semantics:
//   'off'      → neutral (shown unless something else is set to 'include')
//   'include'  → if ANY tag is 'include', only included types render
//   'exclude'  → always hidden
export type MsgType = 'assistant' | 'user' | 'tool' | 'mcp' | 'system' | 'result';
export type TagState = 'off' | 'include' | 'exclude';
export const MSG_TYPES: { id: MsgType; label: string; role: string }[] = [
	{ id: 'assistant', label: 'Assistant', role: 'assistant' },
	{ id: 'user', label: 'User', role: 'user' },
	{ id: 'tool', label: 'Tools', role: 'tool' },
	{ id: 'mcp', label: 'MCP', role: 'mcp' },
	{ id: 'system', label: 'System', role: 'system' },
	{ id: 'result', label: 'Results', role: 'result' }
];

export interface ViewOpts {
	// Per-type tag filter state (CCT-250 item 2).
	typeFilter: Record<MsgType, TagState>;
	// Formatting toggles (kept as toggles, visually grouped).
	prettyJson: boolean;
	prettyDiff: boolean;
	prettyTables: boolean;
	// Desktop drawer width in px (drag-to-resize the left border). Null → the
	// default min(900px, 100vw). Persisted with the other view opts.
	paneWidth: number | null;
}

export interface AskQuestion {
	header?: string;
	question: string;
	multiSelect?: boolean;
	options: { label: string; description?: string; preview?: string }[];
}

export interface Line {
	role: 'assistant' | 'user' | 'system' | 'tool' | 'result' | 'reset' | 'compact';
	ts: number;
	html?: string;
	// Pre-highlighted code HTML for the <pre> bubble (tool/result), {@html}.
	htmlCode?: string;
	text?: string;
	// Code language for tool input (sh/json/diff/…), used to fence the
	// copy-as-Markdown output (CCT-297 #17).
	lang?: string;
	tool?: string;
	// Tool calls under the mcp__ prefix get the distinct MCP role hue.
	mcp?: boolean;
	pending?: boolean;
	// Set on a pending user line that auto-retry is currently re-attempting
	// (CCT-214): shows a "retrying (n/m)" hint instead of plain "sending…".
	retrying?: { attempt: number; max: number };
	// Set on a user line whose send failed (CCT-212): the error reason, shown
	// red with a Retry control.
	failed?: string;
	// Parsed AskUserQuestion payload (CCT-146) — rendered as interactive cards.
	ask?: AskQuestion[];
	// Parsed ExitPlanMode plan markdown (CCT-347) — rendered as a Plan card.
	plan?: string;
	durationMs?: number;
	messageId?: string;
	usage?: TokenUsageT;
}
