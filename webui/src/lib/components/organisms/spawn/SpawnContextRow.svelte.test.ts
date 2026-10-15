// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { QueryClient } from '@tanstack/svelte-query';
import { afterEach, expect, it, vi } from 'vitest';
import Host from './SpawnContextRow.host.test.svelte';

vi.mock('$lib/queries', async () => {
	const { createQuery } = await import('@tanstack/svelte-query');
	const item = (id: string, name: string) => ({
		id,
		name,
		title: name,
		kind: 'memory',
		enabled: true,
		tags: []
	});
	return {
		useContextItems: () =>
			createQuery(() => ({
				queryKey: ['context'],
				queryFn: async () => [item('id-pin', 'pinned'), item('id-a', 'mem-a')]
			})),
		useResolvedContext: (coords: () => { adapter?: string }) =>
			createQuery(() => ({
				queryKey: ['context', 'resolve', coords().adapter],
				queryFn: async () => ({ rules: [], memories: [item('id-a', 'mem-a')] })
			}))
	};
});

let component: ReturnType<typeof mount> | undefined;
afterEach(() => {
	if (component) unmount(component);
	component = undefined;
	document.body.innerHTML = '';
});

it('settles when the parent form is spread into the resolve query (profile selected)', async () => {
	const errors: string[] = [];
	const onError = (e: ErrorEvent) => errors.push(String(e.error ?? e.message));
	window.addEventListener('error', onError);
	let form!: { context_items: string[]; context_auto: boolean; pins: string[] };
	try {
		component = mount(Host, {
			target: document.body,
			props: { client: new QueryClient(), onready: (f) => (form = f) }
		});
		await vi.waitFor(() => expect(form.context_items).toEqual(['mem-a']));
		await new Promise((r) => setTimeout(r, 20));
		flushSync();
	} finally {
		window.removeEventListener('error', onError);
	}
	expect(errors).toEqual([]);
	expect(form.context_items).toEqual(['mem-a']);
	expect(form.context_auto).toBe(true);
	expect(form.pins).toEqual(['pinned']);
}, 15_000);
