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
