// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import type { ContextItem } from '@bindings/ContextItem';
import type { SessionContextResponse } from '@bindings/SessionContextResponse';

const items = { data: undefined as ContextItem[] | undefined };
vi.mock('$lib/queries', async () => {
	const { newerVersion } =
		await vi.importActual<typeof import('$lib/queries/context')>('$lib/queries/context');
	return { newerVersion, useContextItems: () => items, useAttachContext: () => vi.fn() };
});
vi.mock('$lib/toast.svelte', () => ({ toasts: { ok: vi.fn(), error: vi.fn() } }));

import SessionContextPanel from './SessionContextPanel.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const ref = (id: string, kind: string, title: string, version: number) => ({
	id,
	kind,
	name: title.toLowerCase(),
	title,
	version
});
const attached: SessionContextResponse = {
	rules: [ref('r1', 'rule', 'House', 3)],
	memories: [ref('m1', 'memory', 'Notes', 1)]
};
const rows = () =>
	[...document.querySelectorAll('.group li')].map((li) =>
		[...li.children]
			.map((c) => c.textContent?.trim())
			.filter(Boolean)
			.join(' ')
	);

describe('SessionContextPanel', () => {
	it('shows the version each item was delivered at, not the current one', () => {
		items.data = undefined;
		comp = mount(SessionContextPanel, {
			target: document.body,
			props: { sessionId: 's1', attached, canAttach: false }
		});
		expect(rows()).toEqual(['House v3', 'Notes v1']);
		expect(document.querySelector('[data-part="newer"]')).toBeNull();
	});

	it('flags an item whose owner has moved it past the delivered version', () => {
		items.data = [
			{ id: 'r1', kind: 'rule', version: 3 },
			{ id: 'm1', kind: 'memory', version: 4 }
		] as ContextItem[];
		comp = mount(SessionContextPanel, {
			target: document.body,
			props: { sessionId: 's1', attached, canAttach: true }
		});
		const flags = [...document.querySelectorAll('[data-part="newer"]')];
		expect(flags.map((f) => f.textContent?.trim())).toEqual(['v4 available']);
		expect(flags[0].closest('li')?.textContent).toContain('Notes');
		expect(rows()[0]).toBe('House v3');
	});
});
