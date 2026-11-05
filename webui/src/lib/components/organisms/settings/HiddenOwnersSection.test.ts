// @vitest-environment happy-dom
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { HiddenOwner } from '@bindings/HiddenOwner';
import { m } from '$lib/paraglide/messages';

const query = vi.hoisted(() => ({
	data: undefined as { owners: HiddenOwner[] } | undefined,
	isPending: true
}));
const actions = vi.hoisted(() => ({
	hide: vi.fn(async () => undefined),
	unhide: vi.fn(async () => undefined)
}));
const enabledArg = vi.hoisted(() => ({ fn: null as (() => boolean) | null }));
vi.mock('$lib/queries', () => ({
	useHiddenOwners: (enabled: () => boolean) => {
		enabledArg.fn = enabled;
		return query;
	},
	useHiddenOwnerActions: () => actions
}));

import HiddenOwnersSection from './HiddenOwnersSection.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	vi.clearAllMocks();
});

function render(owners: HiddenOwner[] | undefined, isAdmin = true) {
	Object.assign(query, { data: owners && { owners }, isPending: owners === undefined });
	comp = mount(HiddenOwnersSection, { target: document.body, props: { isAdmin } });
	flushSync();
}

const rows = () => [...document.querySelectorAll<HTMLElement>('[data-journey="hidden-owner"]')];

describe('Settings › hidden users and machines', () => {
	it('renders nothing for a non-admin and never enables the query', () => {
		render([{ kind: 'user', target_id: 'u-bob', label: 'bob' }], false);
		expect(document.querySelector('#hidden-owners')).toBeNull();
		expect(enabledArg.fn?.()).toBe(false);
	});

	it('lists each hidden owner with its kind, machines first', () => {
		render([
			{ kind: 'user', target_id: 'u-bob', label: 'bob' },
			{ kind: 'machine', target_id: 'm-1', label: 'workbench' }
		]);
		expect(document.querySelector('#hidden-owners')).not.toBeNull();
		expect(rows().map((r) => r.dataset.journeyKey)).toEqual(['machine:m-1', 'user:u-bob']);
		expect(rows()[0].textContent).toContain('workbench');
		expect(rows()[0].textContent).toContain(m.hidden_owners_kind_machine());
		expect(rows()[1].textContent).toContain(m.hidden_owners_kind_user());
	});

	it('shows the empty state when nothing is hidden', () => {
		render([]);
		expect(document.querySelector('[data-journey="hidden-owners-empty"]')).not.toBeNull();
		expect(rows()).toHaveLength(0);
	});

	it('unhides the clicked owner', async () => {
		render([{ kind: 'user', target_id: 'u-bob', label: 'bob' }]);
		const btn = document.querySelector<HTMLButtonElement>(
			`button[aria-label="${m.hidden_owners_unhide_named({ name: 'bob' })}"]`
		);
		expect(btn).not.toBeNull();
		btn!.click();
		await tick();
		expect(actions.unhide).toHaveBeenCalledWith('user', 'u-bob');
		expect(actions.hide).not.toHaveBeenCalled();
	});

	it('labels an owner the server could not name by its id', () => {
		render([{ kind: 'machine', target_id: 'm-gone', label: null }]);
		expect(rows()[0].textContent).toContain('m-gone');
	});
});
