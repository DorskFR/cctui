// Shared types + constants for the conversation drawer and its sub-components,
// extracted from ConversationDrawer.svelte (no behavior change).
import type { TokenUsage as TokenUsageT } from '@bindings/TokenUsage';
import { m } from '$lib/paraglide/messages';

// ── Message-type tag filter ──────────────────────────────
// Each message type is a clickable badge with include/exclude semantics:
//   'off'      → neutral (shown unless something else is set to 'include')
//   'include'  → if ANY tag is 'include', only included types render
//   'exclude'  → always hidden
export type MsgType =
	| 'assistant'
	| 'thinking'
	| 'user'
	| 'tool'
	| 'mcp'
	| 'system'
	| 'result'
	| 'summary';
export type TagState = 'off' | 'include' | 'exclude';
export const MSG_TYPES: { id: MsgType }[] = [
	{ id: 'assistant' },
	{ id: 'thinking' },
	{ id: 'user' },
	{ id: 'tool' },
	{ id: 'mcp' },
	{ id: 'system' },
	{ id: 'result' },
	{ id: 'summary' }
];

// Resolved at call time so a live language switch re-renders the badges.
export function msgTypeLabel(id: MsgType): string {
	switch (id) {
		case 'assistant':
			return m.conversation_filter_assistant();
		case 'thinking':
			return m.conversation_filter_thinking();
		case 'user':
			return m.conversation_filter_user();
		case 'tool':
			return m.conversation_filter_tool();
		case 'mcp':
			return m.conversation_filter_mcp();
		case 'system':
			return m.conversation_filter_system();
		case 'result':
			return m.conversation_filter_result();
		case 'summary':
			return m.conversation_filter_summary();
	}
}

export interface ViewOpts {
	// Per-type tag filter state.
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

// Post-turn summary emitted by the server at turn end. Rendered as a footer on
// the turn's last assistant bubble, never as a bubble of its own.
export interface TurnSummary {
	detail: string;
	needsAction: boolean;
	ts: number;
}

export interface Line {
	role:
		| 'assistant'
		| 'thinking'
		| 'user'
		| 'system'
		| 'tool'
		| 'result'
		| 'reset'
		| 'compact'
		| 'summary';
	ts: number;
	html?: string;
	// Pre-highlighted code HTML for the <pre> bubble (tool/result), {@html}.
	htmlCode?: string;
	text?: string;
	// Code language for tool input (sh/json/diff/…), used to fence the
	// copy-as-Markdown output.
	lang?: string;
	tool?: string;
	// Tool calls under the mcp__ prefix get the distinct MCP role hue.
	mcp?: boolean;
	// Thinking whose content the provider withheld: same brown treatment, dimmed.
	redacted?: boolean;
	pending?: boolean;
	// Set on a pending user line that auto-retry is currently re-attempting
	//: shows a "retrying (n/m)" hint instead of plain "sending…".
	retrying?: { attempt: number; max: number };
	// Set on a user line whose send failed: the error reason, shown
	// red with a Retry control.
	failed?: string;
	// Parsed AskUserQuestion payload — rendered as interactive cards.
	ask?: AskQuestion[];
	// Parsed ExitPlanMode plan markdown — rendered as a Plan card.
	plan?: string;
	// Turn summary attached to this (assistant) line, rendered under its bubble.
	summary?: TurnSummary;
	durationMs?: number;
	key?: string;
	// 1-based conversation turn; stamped only on assistant lines.
	turn?: number;
	messageId?: string;
	usage?: TokenUsageT;
}
