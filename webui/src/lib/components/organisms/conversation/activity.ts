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
