// Pure formatting / parsing / dedup helpers for the conversation drawer,
// extracted from ConversationDrawer.svelte (no behavior change). Everything
// here is side-effect free — view-dependent formatting takes its toggles as
// explicit args so these stay testable and decoupled from component state.
import type { AgentEvent } from '@bindings/AgentEvent';
import { userMsgKey } from '$lib/ws.svelte';
import { prettyJson } from '$lib/markdown';
import { m } from '$lib/paraglide/messages';
import type { AskQuestion, Line } from './types';

// Some "user" turns are really harness/system messages directed at the agent
// (timer wake-ups, task-completion notifications, injected reminders, skill
// preambles, hook feedback) rather than something the human typed. We classify
// these structurally, by the fixed marker the harness/Claude prefixes them with.
//
// We deliberately ignore the stored `meta` bit (Claude's `isMeta`): cctui
// delivers a human's composer reply through Claude's control-socket `reply` op,
// which Claude records `isMeta:true`, so trusting it reclassified genuine human
// turns to `system` and made them appear to vanish on reload. Keep
// this list in sync with `META_MARKERS` in the daemon's transcript parser.
export const META_TAGS = [
	'<task-notification',
	'<system-reminder',
	'<command-name',
	'<command-message',
	'<local-command',
	'<bash-input',
	'<bash-stdout',
	'<bash-stderr',
	'[SYSTEM NOTIFICATION',
	'Base directory for this skill:',
	'Stop hook feedback:',
	'# Autonomous loop'
];
export function looksMeta(text: string): boolean {
	const t = text.trimStart();
	return META_TAGS.some((m) => t.startsWith(m));
}

// Pull a well-formed questions[] out of an AskUserQuestion tool input.
export function parseAsk(input: unknown): AskQuestion[] | null {
	const qs = (input as { questions?: unknown })?.questions;
	if (!Array.isArray(qs) || qs.length === 0) return null;
	const out = qs
		.filter(
			(q): q is AskQuestion =>
				!!q && typeof (q as AskQuestion).question === 'string' && Array.isArray((q as AskQuestion).options)
		)
		.map((q) => ({
			header: q.header,
			question: q.question,
			multiSelect: !!q.multiSelect,
			options: q.options.map((o) => ({ label: String(o.label ?? ''), description: o.description, preview: o.preview }))
		}));
	return out.length ? out : null;
}

// Pull the plan markdown out of an ExitPlanMode tool input. The
// peer of `parseAsk` — used to render a historic plan tool_call as a Plan card.
export function parsePlan(input: unknown): string | null {
	const plan = (input as { plan?: unknown })?.plan;
	if (typeof plan !== 'string' || plan.trim().length === 0) return null;
	return plan;
}

// Content signature of an event, used to dedup the live stream against fetched
// history (the same logical event has a DIFFERENT `ts` in each source — history
// stamps DB `created_at`, live carries the daemon ts — so ts can't be the key).
// User messages collapse across their three shapes via `userMsgKey`. Markers
// (reset/turn_end/heartbeat) key on ts so distinct ones aren't over-collapsed.
export function eventSig(e: AgentEvent): string {
	const u = userMsgKey(e);
	if (u !== null) return `u:${u}`;
	switch (e.type) {
		case 'text':
			return `a:${e.content.trim()}`;
		case 'tool_call':
			return `tc:${e.tool}:${JSON.stringify(e.input)}`;
		case 'tool_result':
			return `tr:${e.tool}:${e.output_summary}`;
		case 'compact_summary':
			return `cs:${e.content.trim()}`;
		default:
			return `${e.type}:${e.ts}`;
	}
}

// Order the merged history+live event list causally. `seq` is the server's
// monotonic per-session insert sequence (`stream_events.id`), stamped on both
// the reload payload and the live broadcast, so it reflects true causal order
// even when receive-time `ts` ties or inverts — a late-flushed AskUserQuestion
// card+preamble carry a `ts` at/after the user's answer but a LOWER `seq`, so
// ordering by `seq` renders the ask before its answer. Falls back to `ts`
// when either event lacks a `seq`. Uses a stable sort so equal keys keep
// history-before-live order.
export function orderEvents(events: AgentEvent[]): AgentEvent[] {
	return [...events].sort((a, b) => {
		const as = a.seq;
		const bs = b.seq;
		if (as !== null && as !== undefined && bs !== null && bs !== undefined) {
			return Number(as) - Number(bs);
		}
		return Number(a.ts) - Number(b.ts);
	});
}

// Stamp each assistant line with its 1-based conversation turn. A
// turn opens on each user/system prompt to the agent; every assistant line up
// to the next prompt shares it. A `/clear` reset (role 'reset') restarts the
// counter; a `/compact` summary does not. Derived from role transitions, not
// raw index, so out-of-`ts` reloads stay correct. Mutates in place.
export function stampTurns(lines: Line[]): Line[] {
	let turn = 0;
	for (const ln of lines) {
		if (ln.role === 'reset') turn = 0;
		else if (ln.role === 'user' || ln.role === 'system') turn++;
		else if (ln.role === 'assistant') {
			if (turn === 0) turn = 1;
			ln.turn = turn;
		}
	}
	return lines;
}

export function assignLineKeys(lines: Line[]): Line[] {
	const counts = new Map<string, number>();
	for (const ln of lines) {
		const base = `${ln.ts}|${ln.role}|${(ln.text ?? ln.html ?? '').slice(0, 24)}`;
		const n = counts.get(base) ?? 0;
		counts.set(base, n + 1);
		ln.key = n === 0 ? base : `${base}#${n}`;
	}
	return lines;
}

// JSON.stringify only emits \n / \t inside string literals, so expanding them
// for display is safe (display-only — the text is never parsed back).
export function expandJsonEscapes(s: string): string {
	return s.replace(/\\n/g, '\n').replace(/\\t/g, '\t');
}

export function formatToolInput(
	tool: string,
	input: unknown,
	opts: { prettyDiff: boolean; prettyJson: boolean }
): { text: string; lang: string } {
	const obj = input as Record<string, unknown> | null;
	if (opts.prettyDiff && obj && typeof obj === 'object' && 'old_string' in obj && 'new_string' in obj) {
		const minus = String(obj.old_string ?? '')
			.split('\n')
			.map((l) => `- ${l}`)
			.join('\n');
		const plus = String(obj.new_string ?? '')
			.split('\n')
			.map((l) => `+ ${l}`)
			.join('\n');
		return { text: `${obj.file_path ?? ''}\n${minus}\n${plus}`.trim(), lang: '' };
	}
	// Shell-ish tools (Bash, BashOutput, …): render the command itself as a
	// shell block with the description as a leading comment, instead of a
	// one-line JSON blob full of literal "\n" escapes.
	if (opts.prettyJson && obj && typeof obj === 'object' && typeof obj.command === 'string') {
		const desc =
			typeof obj.description === 'string' && obj.description.trim()
				? `# ${obj.description.trim()}\n`
				: '';
		return { text: `${desc}${obj.command}`, lang: 'sh' };
	}
	if (!opts.prettyJson) return { text: JSON.stringify(input), lang: 'json' };
	// Expand escaped newlines/tabs inside string values so multiline payloads
	// (scripts, file contents, heredocs) read as real lines rather than one long
	// "…\n…" run before the highlighter sees them.
	return { text: expandJsonEscapes(prettyJson(input)), lang: 'json' };
}

// Render a single message line as Markdown. Assistant/user/system
// content is already a Markdown source string, so it copies verbatim; tool/result
// code is wrapped in a fenced block (with the tool's language when known).
export function lineMarkdown(ln: Line): string {
	const t = ln.text ?? '';
	if (ln.role === 'tool') {
		const label = ln.tool ? `**${ln.mcp ? 'MCP' : m.turn_tool_label()} · ${ln.tool}**\n\n` : '';
		return `${label}\`\`\`${ln.lang ?? ''}\n${t}\n\`\`\``;
	}
	if (ln.role === 'result') {
		const label = ln.tool ? `**${m.turn_result_label()} · ${ln.tool}**\n\n` : '';
		return `${label}\`\`\`\n${t}\n\`\`\``;
	}
	return t;
}
