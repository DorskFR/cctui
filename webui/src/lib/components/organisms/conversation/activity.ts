import type { AgentEvent } from '@bindings/AgentEvent';

const SUMMARY_MAX = 140;

export interface ActivityTool {
	tool: string;
	summary: string;
	startedAt: number;
}

export function truncate(s: string, max = SUMMARY_MAX): string {
	const t = s.replace(/\s+/g, ' ').trim();
	return t.length > max ? `${t.slice(0, max - 1)}…` : t;
}

// Ordered by specificity: the first hit wins, so `command` must precede the
// generic fallbacks below.
const SUMMARY_KEYS = [
	'command',
	'file_path',
	'notebook_path',
	'path',
	'url',
	'pattern',
	'query',
	'prompt',
	'description',
	'subagent_type',
	'skill',
	'to'
];

export function toolInvocationSummary(tool: string, input: unknown): string {
	if (!input || typeof input !== 'object') return '';
	const obj = input as Record<string, unknown>;
	for (const k of SUMMARY_KEYS) {
		const v = obj[k];
		if (typeof v === 'string' && v.trim()) return truncate(k === 'command' ? `$ ${v}` : v);
	}
	for (const v of Object.values(obj)) {
		if (typeof v === 'string' && v.trim() && v.length <= 200) return truncate(v);
	}
	return '';
}

export function formatElapsed(ms: number): string {
	const s = Math.max(0, Math.floor(ms / 1000));
	if (s < 60) return `${s}s`;
	const m = Math.floor(s / 60);
	if (m < 60) return `${m}m ${s % 60}s`;
	return `${Math.floor(m / 60)}h ${m % 60}m`;
}

export function lastProseLine(content: string): string | null {
	const lines = content
		.split('\n')
		.map((l) => l.trim())
		.filter(Boolean);
	const last = lines.at(-1);
	return last ? truncate(last) : null;
}

/** Whether the agent is mid-turn after `ev`. Only agent work starts a turn:
 *  meta lines and summaries trail `turn_end`, and must not reopen it. */
export function workingAfter(working: boolean, ev: AgentEvent): boolean {
	switch (ev.type) {
		case 'turn_end':
			return false;
		case 'heartbeat':
		case 'turn_summary':
		case 'compact_summary':
		case 'context_reset':
			return working;
		case 'text':
			return ev.meta ? working : true;
		default:
			return true;
	}
}

/** The newer of `at` and `ev`'s time when `ev` is a turn's traffic (prose,
 *  tools, a user message), the moments that touch the prompt cache. */
export function agentActivityAt(at: number | null, ev: AgentEvent): number | null {
	const counts =
		(ev.type === 'text' && !ev.meta) ||
		ev.type === 'tool_call' ||
		ev.type === 'tool_result' ||
		ev.type === 'reply';
	if (!counts || typeof ev.ts !== 'number') return at;
	return at === null ? ev.ts : Math.max(at, ev.ts);
}
