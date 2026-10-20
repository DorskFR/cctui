import type { ContextPackGrant } from '@bindings/ContextPackGrant';
import type { DispatchPolicyRules } from '@bindings/DispatchPolicyRules';

/** The policy editor's state: lists as free text, limits as raw input. */
export interface PolicyForm {
	dispatchers: string;
	profiles: string;
	adapters: string;
	models: string;
	packs: ContextPackGrant[];
	maxTimeout: string;
	maxConcurrent: string;
	dailyCap: string;
}

export const blankPack = (): ContextPackGrant => ({ name: '', url: '', ref: '', subdir: '' });

/** Comma, whitespace or newline separated names: trimmed, emptied, deduped. */
export function parseList(text: string): string[] {
	const out: string[] = [];
	for (const raw of text.split(/[\s,]+/)) {
		const s = raw.trim();
		if (s && !out.includes(s)) out.push(s);
	}
	return out;
}

/** A positive integer, else unset (blank, zero, negative and junk all clear it). */
export function parseLimit(text: string): number | undefined {
	const n = Number(text.trim());
	return Number.isInteger(n) && n > 0 ? n : undefined;
}

const optional = (s: string | undefined): string | undefined => {
	const t = (s ?? '').trim();
	return t ? t : undefined;
};

export function formFromRules(rules: DispatchPolicyRules | null | undefined): PolicyForm {
	return {
		dispatchers: (rules?.dispatchers ?? []).join(', '),
		profiles: (rules?.profiles ?? []).join(', '),
		adapters: (rules?.adapters ?? []).join(', '),
		models: (rules?.models ?? []).join(', '),
		packs: (rules?.context_packs ?? []).map((p) => ({
			name: p.name,
			url: p.url,
			ref: p.ref ?? '',
			subdir: p.subdir ?? ''
		})),
		maxTimeout: rules?.max_timeout_minutes?.toString() ?? '',
		maxConcurrent: rules?.max_concurrent?.toString() ?? '',
		dailyCap: rules?.daily_cap?.toString() ?? ''
	};
}

/** Packs with neither a name nor a url are editor leftovers, not rules. */
export function rulesFromForm(form: PolicyForm): DispatchPolicyRules {
	const context_packs = form.packs
		.filter((p) => p.name.trim() || p.url.trim())
		.map((p) => ({
			name: p.name.trim(),
			url: p.url.trim(),
			ref: optional(p.ref),
			subdir: optional(p.subdir)
		}));
	return {
		dispatchers: parseList(form.dispatchers),
		profiles: parseList(form.profiles),
		adapters: parseList(form.adapters),
		models: parseList(form.models),
		context_packs,
		max_timeout_minutes: parseLimit(form.maxTimeout),
		max_concurrent: parseLimit(form.maxConcurrent),
		daily_cap: parseLimit(form.dailyCap)
	};
}

/** A pack the server would refuse: one of name/url without the other. */
export function incompletePacks(form: PolicyForm): boolean {
	return form.packs.some((p) => !!p.name.trim() !== !!p.url.trim());
}
