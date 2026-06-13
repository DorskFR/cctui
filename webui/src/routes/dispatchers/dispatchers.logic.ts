// Pure helpers for the dispatchers page — no Svelte/reactive state, so they live
// outside the component and are unit-testable on their own.
import type { UserDispatcher } from '$lib/queries';

/** One-line summary of a dispatcher's config, shown in the table's Config column. */
export function summarize(d: UserDispatcher): string {
	const c = d.config ?? {};
	if (d.kind === 'http') {
		const tok = c.token ? ' · token set' : '';
		return `${(c.url as string) ?? ''}${tok}`;
	}
	return `${(c.namespace as string) ?? ''}/${(c.source_cronjob as string) ?? ''}`;
}
