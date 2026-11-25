// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('#lib/queries/index.ts', () => ({ useMachineResources: () => ({ data: [] }) }));

import { DEFAULT_FILTERS } from '#lib/events.ts';
import { m } from '#lib/paraglide/messages.js';
import EventFilterBar from './EventFilterBar.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

describe('EventFilterBar', () => {
	it('names each pill group with a visible label', () => {
		comp = mount(EventFilterBar, { target: document.body, props: { filters: { ...DEFAULT_FILTERS } } });
		flushSync();
		const groups = [...document.body.querySelectorAll('.bar [role="group"]')];
		const names = groups.map((g) => {
			const id = g.getAttribute('aria-labelledby') ?? '';
			return document.getElementById(id)?.textContent?.trim();
		});
		expect(names).toEqual([m.events_filter_kind(), m.events_filter_severity()]);
	});
});
