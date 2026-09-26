import type { ToolPolicy } from '@bindings/ToolPolicy';

export const POLICY_FIELDS = ['terms', 'patterns', 'protected_owners', 'exempt_roots'] as const;

export type PolicyDraft = Record<(typeof POLICY_FIELDS)[number], string>;

export function toDraft(p: ToolPolicy | undefined): PolicyDraft {
	return {
		terms: (p?.terms ?? []).join('\n'),
		patterns: (p?.patterns ?? []).join('\n'),
		protected_owners: (p?.protected_owners ?? []).join('\n'),
		exempt_roots: (p?.exempt_roots ?? []).join('\n')
	};
}

const lines = (text: string) =>
	text
		.split('\n')
		.map((l) => l.trim())
		.filter((l) => l.length > 0);

export function fromDraft(d: PolicyDraft): ToolPolicy {
	return {
		terms: lines(d.terms),
		patterns: lines(d.patterns),
		protected_owners: lines(d.protected_owners),
		exempt_roots: lines(d.exempt_roots)
	};
}

export const sameDraft = (a: PolicyDraft, b: PolicyDraft) =>
	POLICY_FIELDS.every((k) => lines(a[k]).join('\n') === lines(b[k]).join('\n'));

/** Exempt roots alone block nothing: the scan is on only with a rule. */
export const policyActive = (p: ToolPolicy) =>
	p.terms.length + p.patterns.length + p.protected_owners.length > 0;
