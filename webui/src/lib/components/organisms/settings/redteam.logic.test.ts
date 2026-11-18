import { describe, expect, it } from 'vitest';
import type { RedteamCell } from '@bindings/RedteamCell';
import type { RedteamVersion } from '@bindings/RedteamVersion';
import { cellsEscapedFirst, escapeLabel, outcomeTone, probeSummary, versionRuns } from './redteam.logic';

const cell = (name: string, verdict: RedteamCell['verdict']): RedteamCell => ({
	cell: name,
	verdict,
	sink_hits: 0,
	probes: [{ id: 'EGR-01', verdict: 'blocked' }],
	reasons: []
});

describe('redteam logic', () => {
	it('puts escaped cells first and keeps run order otherwise', () => {
		const sorted = cellsEscapedFirst([cell('a', 'contained'), cell('b', 'escaped'), cell('c', 'inconclusive')]);
		expect(sorted.map((c) => c.cell)).toEqual(['b', 'a', 'c']);
	});

	it('labels an escape by probe and cell, or as run-level', () => {
		expect(escapeLabel({ cell: 'codex/kube', probe: 'EGR-04' })).toBe('EGR-04 · codex/kube');
		expect(escapeLabel({ cell: 'codex/kube', probe: null })).toBe('codex/kube');
		expect(escapeLabel({ cell: '', probe: null })).not.toBe('');
	});

	it('tones escapes as danger and containment as ok', () => {
		expect(outcomeTone('escaped')).toBe('danger');
		expect(outcomeTone('contained')).toBe('ok');
		expect(outcomeTone('blocked')).toBe('ok');
		expect(outcomeTone('inconclusive')).toBe('warn');
	});

	it('summarises probes and orders the T2 run before the T3 lanes', () => {
		expect(probeSummary(cell('a', 'contained'))).toContain('EGR-01');
		const run = (lane: string) => ({
			id: 1,
			received_at: 0,
			posted_by: null,
			escapes: [],
			report: {
				schema: 1,
				tier: lane ? ('t3' as const) : ('t2' as const),
				lane,
				run_id: 'r',
				version: '1.0.0',
				verdict: 'contained' as const,
				model: null,
				started_at: null,
				finished_at: 0,
				run_url: null,
				cells: [],
				reasons: []
			}
		});
		const v: RedteamVersion = { version: '1.0.0', t2: run(''), t3: [run('claude-code'), run('codex')] };
		expect(versionRuns(v).map((r) => r.key)).toEqual(['t2', 't3:claude-code', 't3:codex']);
	});
});
