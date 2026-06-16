// Pure formatting / parsing / dedup helpers for the conversation drawer,
// extracted from ConversationDrawer.svelte (no behavior change). Everything
// here is side-effect free — view-dependent formatting takes its toggles as
// explicit args so these stay testable and decoupled from component state.
import type { AgentEvent } from '@bindings/AgentEvent';
import { userMsgKey } from '$lib/ws.svelte';
import { prettyJson } from '$lib/markdown';
import type { AskQuestion, Line } from './types';

// Some "user" turns are really harness/system messages directed at the agent
// (timer wake-ups, task-completion notifications, injected reminders) rather
// than something the human typed. The adapter layer marks these authoritatively
// via `meta`; this tag fallback only covers events stored before `meta` existed.
export const META_TAGS = [
	'<task-notification',
	'<system-reminder',
	'<command-name',
	'<command-message',
	'<local-command',
	'<bash-input',
	'<bash-stdout',
	'<bash-stderr'
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

// Pull the plan markdown out of an ExitPlanMode tool input (CCT-347). The
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

// Re-anchor an AskUserQuestion turn to its causal narrative order (CCT-338).
//
// Claude flushes the AskUserQuestion tool_use block (and the assistant prose
// preamble preceding it) only AFTER the turn advances — i.e. after the user has
// already answered. Live events and DB rows are ordered by receive-time `ts`
// (the server stamps `Utc::now()` on ingest; see normalize.rs), NOT by the
// original transcript order. So once history is (re)fetched or the page is
// reloaded, the persisted preamble + ask card sort AFTER the user's answer,
// inverting the narrative:
//   [answer] · [preamble] · [ask card] · [continuation]   (wrong)
// The correct, durable order is:
//   [preamble] · [ask card] · [answer] · [continuation]
//
// This is a pure, idempotent transform over the BUILT lines: a contiguous
// `[preamble?, ask card]` block that is immediately preceded by a user/system
// answer line is lifted to before that answer. When the order is already
// correct (the live case, where preamble+card render before the optimistic
// reply), no block matches and the input is returned unchanged.
//
// The preamble is the assistant prose immediately preceding the ask card; the
// ask card is the only durable anchor we can match across sources (its question
// text survives refetch/reload), so we key the block off the card and absorb at
// most the assistant lines directly above it as its preamble.
export function orderAskTurns(lines: Line[]): Line[] {
	// Index of the answer that precedes an ask block, if the block is inverted.
	const isAskCard = (l: Line) => l.role === 'tool' && !!l.ask;
	const isAnswer = (l: Line) => l.role === 'user' || l.role === 'system';
	const out = lines.slice();
	for (let i = 0; i < out.length; i++) {
		if (!isAskCard(out[i])) continue;
		// Absorb the contiguous assistant preamble run directly above the card.
		let start = i;
		while (start > 0 && out[start - 1].role === 'assistant') start--;
		// The block is [start .. i]; it is inverted iff the line directly above it
		// is the user's answer (the answer landed before the late preamble + card).
		const answerIdx = start - 1;
		if (answerIdx < 0 || !isAnswer(out[answerIdx])) continue;
		// Lift the block to before the answer (stable: relative order preserved).
		const block = out.splice(start, i - start + 1);
		out.splice(answerIdx, 0, ...block);
		// Continue past the moved block (now ending where the answer used to be).
		i = answerIdx + block.length;
	}
	return out;
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
	// Shell-ish tools (Bash, BashOutput, …): render the command itself as a shell
	// block with the description as a leading comment, instead of a one-line JSON
	// blob full of literal "\n" escapes — those escapes were the "weird artifacts"
	// / un-prettified commands (CCT-161 cleanup).
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

// Render a single message line as Markdown (CCT-297 #17). Assistant/user/system
// content is already a Markdown source string, so it copies verbatim; tool/result
// code is wrapped in a fenced block (with the tool's language when known).
export function lineMarkdown(ln: Line): string {
	const t = ln.text ?? '';
	if (ln.role === 'tool') {
		const label = ln.tool ? `**${ln.mcp ? 'MCP' : 'Tool'} · ${ln.tool}**\n\n` : '';
		return `${label}\`\`\`${ln.lang ?? ''}\n${t}\n\`\`\``;
	}
	if (ln.role === 'result') {
		const label = ln.tool ? `**Result · ${ln.tool}**\n\n` : '';
		return `${label}\`\`\`\n${t}\n\`\`\``;
	}
	return t;
}
