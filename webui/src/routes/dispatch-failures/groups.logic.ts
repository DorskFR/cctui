import type { FailedDispatch } from '@bindings/FailedDispatch';
import type { RerunBlocker } from '@bindings/RerunBlocker';

export type CauseKind = RerunBlocker | 'rerunnable' | 'resolved';

export type CauseGroup = {
	key: string;
	kind: CauseKind;
	/** The env keys every member lacks; only for `env`. */
	envKeys: string[];
	rows: FailedDispatch[];
};

const ORDER: CauseKind[] = [
	'rerunnable',
	'env',
	'prompt_truncated',
	'prompt_redacted',
	'payload_too_large',
	'resolved'
];

export function causeOf(d: FailedDispatch): { key: string; kind: CauseKind; envKeys: string[] } {
	if (d.superseded_by) return { key: 'resolved', kind: 'resolved', envKeys: [] };
	const kind: CauseKind = d.rerun_blocked ?? 'rerunnable';
	if (kind !== 'env') return { key: kind, kind, envKeys: [] };
	const envKeys = [...new Set(d.env_keys ?? [])].sort();
	return { key: `env:${envKeys.join(',')}`, kind, envKeys };
}

/** Groups keep the input (newest-first) order inside; groups sort by cause, then by first appearance. */
export function groupByCause(rows: FailedDispatch[]): CauseGroup[] {
	const groups = new Map<string, CauseGroup>();
	for (const d of rows) {
		const c = causeOf(d);
		const g = groups.get(c.key);
		if (g) g.rows.push(d);
		else groups.set(c.key, { ...c, rows: [d] });
	}
	return [...groups.values()]
		.map((g, i) => ({ g, i }))
		.sort((a, b) => ORDER.indexOf(a.g.kind) - ORDER.indexOf(b.g.kind) || a.i - b.i)
		.map(({ g }) => g);
}
