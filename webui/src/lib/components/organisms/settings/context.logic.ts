import type { ContextItem } from '@bindings/ContextItem';
import type { ContextItemSpec } from '@bindings/ContextItemSpec';

export const CONTEXT_KINDS = ['memory', 'prompt'] as const;
export const CONTEXT_SCOPES = ['user', 'machine', 'path', 'label'] as const;

export type ContextKind = (typeof CONTEXT_KINDS)[number];
export type ContextScope = (typeof CONTEXT_SCOPES)[number];

/** Mirrors the server's `clean_name`: a slug that is also a safe filename. */
const NAME_RE = /^[a-z0-9][a-z0-9-]{0,63}$/;

export function newContextItem(kind: ContextKind): ContextItemSpec {
	return {
		kind,
		name: '',
		title: '',
		body: '',
		scope: 'user',
		scope_ref: null,
		tags: [],
		enabled: true
	};
}

export function toSpec(item: ContextItem): ContextItemSpec {
	return {
		kind: item.kind,
		name: item.name,
		title: item.title,
		body: item.body,
		scope: item.scope,
		scope_ref: item.scope_ref,
		tags: item.tags,
		enabled: item.enabled
	};
}

/** Suggest a slug from a title, so the name field rarely needs typing. */
export function slugify(title: string): string {
	return title
		.toLowerCase()
		.normalize('NFD')
		.replace(/[̀-ͯ]/g, '')
		.replace(/[^a-z0-9]+/g, '-')
		.replace(/^-+|-+$/g, '')
		.slice(0, 64)
		.replace(/-+$/, '');
}

/**
 * Everything wrong with a draft, as message keys the section maps to copy.
 * Mirrors the server's validation so the form refuses before the round trip.
 */
export function contextProblems(spec: ContextItemSpec): string[] {
	const out: string[] = [];
	if (!spec.title?.trim()) out.push('title');
	if (!NAME_RE.test(spec.name ?? '')) out.push('name');
	if ((spec.scope ?? 'user') !== 'user' && !spec.scope_ref?.trim()) out.push('scope_ref');
	if (spec.scope === 'path' && spec.scope_ref && !spec.scope_ref.trim().startsWith('/'))
		out.push('abs_path');
	if (!spec.body?.trim()) out.push('body');
	return out;
}

/** One line describing when an item applies, for the list row. */
export function scopeSummary(item: Pick<ContextItem, 'scope' | 'scope_ref'>): string {
	return item.scope === 'user' ? 'always' : `${item.scope}: ${item.scope_ref ?? ''}`;
}
