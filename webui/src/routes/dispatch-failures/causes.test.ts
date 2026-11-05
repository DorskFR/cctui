import { describe, expect, it } from 'vitest';
import type { FailedDispatch } from '@bindings/FailedDispatch';
import { causeKey, groupByCause, promptFileHref, summarizeFailures } from './causes';

let n = 0;
function d(over: Partial<FailedDispatch> = {}): FailedDispatch {
	n++;
	return {
		session_id: `s${n}`,
		dispatcher: 'kube',
		reason: 'never_registered',
		dispatched_at: '2026-11-05T08:00:00Z',
		env_keys: [],
		...over
	} as FailedDispatch;
}

describe('groupByCause', () => {
	it('explains a shared cause once and lists every failure under it', () => {
		const env = { rerun_blocked: 'env' as const, env_keys: ['B', 'A'] };
		const list = [d(env), d({ reason: 'crashed' }), d({ ...env, env_keys: ['A', 'B'] })];
		const groups = groupByCause(list);
		expect(groups.map((g) => g.items.length)).toEqual([2, 1]);
		expect(groups[0].reason).toBe('never started');
		expect(groups[0].explanation).toContain('A, B');
		expect(groups[1].explanation).toBeNull();
	});

	it('keys on reason and blocker only, so env key sets do not split a group', () => {
		expect(causeKey(d({ rerun_blocked: 'env', env_keys: ['A'] }))).toBe(
			causeKey(d({ rerun_blocked: 'env', env_keys: ['B'] }))
		);
		expect(causeKey(d())).not.toBe(causeKey(d({ rerun_blocked: 'prompt_truncated' })));
	});

	it('names the cause in every group so equal reasons read apart', () => {
		const groups = groupByCause([
			d({ reason: 'crashed' }),
			d({ reason: 'crashed', rerun_blocked: 'env', env_keys: ['A'] }),
			d({ reason: 'crashed', rerun_blocked: 'env', env_keys: ['B'] })
		]);
		expect(groups.map((g) => [g.reason, g.cause, g.items.length])).toEqual([
			['crashed', 'can be re-dispatched', 1],
			['crashed', 'env not stored', 2]
		]);
		expect(groups[1].explanation).toContain('A, B');
	});
});

describe('summarizeFailures', () => {
	it('is null with nothing to report', () => {
		expect(summarizeFailures([])).toBeNull();
	});

	it('names the cause only when every failure shares it', () => {
		const same = summarizeFailures([d(), d()]);
		expect(same).toMatchObject({ count: 2, cause: 'never started', causes: 1 });
		const mixed = summarizeFailures([d(), d({ reason: 'refused' })]);
		expect(mixed).toMatchObject({ count: 2, cause: null, causes: 2 });
		expect(mixed?.ids).toHaveLength(2);
	});
});

describe('promptFileHref', () => {
	it('links a prompt file inside a GitHub repository', () => {
		expect(promptFileHref('DorskFR/cctui', 'prompts/triage.md')).toBe(
			'https://github.com/DorskFR/cctui/blob/HEAD/prompts/triage.md'
		);
		expect(promptFileHref('https://github.com/DorskFR/cctui.git', './a b.md')).toBe(
			'https://github.com/DorskFR/cctui/blob/HEAD/a%20b.md'
		);
	});

	it('gives no link when the repo is not a GitHub slug or the path escapes it', () => {
		expect(promptFileHref('cctui', 'a.md')).toBeNull();
		expect(promptFileHref(null, 'a.md')).toBeNull();
		expect(promptFileHref('o/r', '../etc/passwd')).toBeNull();
	});
});
