import { describe, expect, it } from 'vitest';
import { fromDraft, policyActive, sameDraft, toDraft } from './tool-policy.logic';

describe('tool policy draft', () => {
	it('round-trips one entry per line, dropping blanks and padding', () => {
		const draft = toDraft({
			terms: ['acme'],
			patterns: [],
			protected_owners: ['acme', 'acme-corp'],
			exempt_roots: ['/home/u/work']
		});
		expect(draft.protected_owners).toBe('acme\nacme-corp');
		draft.terms = '  acme \n\n secret\n';
		expect(fromDraft(draft)).toEqual({
			terms: ['acme', 'secret'],
			patterns: [],
			protected_owners: ['acme', 'acme-corp'],
			exempt_roots: ['/home/u/work']
		});
	});

	it('treats whitespace-only edits as unchanged', () => {
		const a = toDraft({ terms: ['x'], patterns: [], protected_owners: [], exempt_roots: [] });
		expect(sameDraft(a, { ...a, terms: 'x\n\n' })).toBe(true);
		expect(sameDraft(a, { ...a, terms: 'y' })).toBe(false);
	});

	it('is off with exempt roots only', () => {
		expect(policyActive(fromDraft(toDraft(undefined)))).toBe(false);
		expect(
			policyActive({ terms: [], patterns: [], protected_owners: [], exempt_roots: ['/w'] })
		).toBe(false);
		expect(
			policyActive({ terms: [], patterns: [], protected_owners: ['acme'], exempt_roots: [] })
		).toBe(true);
	});
});
