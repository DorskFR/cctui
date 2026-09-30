import { describe, expect, it } from 'vitest';
import {
	SHARE_CANDIDATE_CAP,
	shareCandidates,
	shareLabel,
	type ShareCandidate
} from './peerShares';

function cand(id: string, over: Partial<ShareCandidate> = {}): ShareCandidate {
	return { id, name: `lane ${id}`, adapter: 'codex', machine: 'box-a', ...over };
}

describe('shareLabel', () => {
	it('falls back to the id and to unknown parts', () => {
		expect(shareLabel(cand('a'))).toBe('lane a (codex on box-a)');
		expect(shareLabel(cand('a', { name: '  ' }))).toBe('a (codex on box-a)');
		expect(shareLabel(cand('a', { name: null, adapter: null, machine: null }))).toBe(
			'a (unknown on unknown machine)'
		);
	});
});

describe('shareCandidates', () => {
	const all = [cand('a'), cand('b'), cand('c')];

	it('never offers the subject itself', () => {
		expect(shareCandidates(all, 'a', new Set(), '').map((c) => c.id)).toEqual(['b', 'c']);
	});

	it('never offers a session already shared', () => {
		expect(shareCandidates(all, 'a', new Set(['b']), '').map((c) => c.id)).toEqual(['c']);
	});

	it('lists everything eligible when nothing is typed', () => {
		expect(shareCandidates(all, 'z', new Set(), '   ')).toHaveLength(3);
	});

	it('matches the name, the adapter and the machine', () => {
		expect(shareCandidates(all, 'z', new Set(), 'lane b').map((c) => c.id)).toEqual(['b']);
		expect(shareCandidates(all, 'z', new Set(), 'codex')).toHaveLength(3);
		expect(shareCandidates(all, 'z', new Set(), 'box-a')).toHaveLength(3);
		expect(shareCandidates(all, 'z', new Set(), 'nothing')).toEqual([]);
	});

	it('matches an id the label does not contain', () => {
		const odd = [cand('7f3e-uuid', { name: 'renamed', adapter: null, machine: null })];
		expect(shareCandidates(odd, 'z', new Set(), '7f3e').map((c) => c.id)).toEqual(['7f3e-uuid']);
	});

	it('is case-insensitive', () => {
		expect(shareCandidates(all, 'z', new Set(), 'LANE B').map((c) => c.id)).toEqual(['b']);
	});

	it('caps the list so the picker stays a search box', () => {
		const many = Array.from({ length: SHARE_CANDIDATE_CAP + 5 }, (_, i) => cand(`s${i}`));
		expect(shareCandidates(many, 'z', new Set(), '')).toHaveLength(SHARE_CANDIDATE_CAP);
	});
});
