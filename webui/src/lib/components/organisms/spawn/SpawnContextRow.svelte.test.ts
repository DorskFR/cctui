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
				queryFn: async () => {
					resolves++;
					return { rules: [], memories: [item('id-a', 'mem-a')] };
				}
			}))
	};
});

let resolves = 0;
let component: ReturnType<typeof mount> | undefined;
afterEach(() => {
	if (component) unmount(component);
	component = undefined;
	document.body.innerHTML = '';
	resolves = 0;
});

it('leaves a matching memory unchecked and settles with a profile selected', async () => {
	const errors: string[] = [];
	const onError = (e: ErrorEvent) => errors.push(String(e.error ?? e.message));
	window.addEventListener('error', onError);
	let form!: { context_items: string[]; pins: string[] };
	try {
		component = mount(Host, {
			target: document.body,
			props: { client: new QueryClient(), onready: (f) => (form = f) }
		});
		await vi.waitFor(() => expect(form.pins).toEqual(['pinned']));
		await vi.waitFor(() => expect(resolves).toBeGreaterThan(0));
		await new Promise((r) => setTimeout(r, 20));
		flushSync();
	} finally {
		window.removeEventListener('error', onError);
	}
	expect(errors).toEqual([]);
	expect(form.context_items).toEqual([]);
	expect(form.pins).toEqual(['pinned']);
}, 15_000);
