// @vitest-environment happy-dom
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { HiddenOwner } from '@bindings/HiddenOwner';

const state = vi.hoisted(() => ({
	owners: [] as HiddenOwner[],
	enabled: [] as (() => boolean)[]
}));
const actions = vi.hoisted(() => ({
	hide: vi.fn(async () => undefined),
	unhide: vi.fn(async () => undefined)
}));
vi.mock('#lib/queries/index.ts', () => {
	const gate = <T>(data: () => T) => (enabled: () => boolean) => {
		state.enabled.push(enabled);
		return {
			get data() {
				return enabled() ? data() : undefined;
			}
		};
	};
	return {
		useMe: () => ({ data: { user_id: 'u-me', role: 'admin' } }),
		useUsers: gate(() => [
			{ id: 'u-me', name: 'me', created_at: '', revoked_at: null, disabled_at: null, can_dispatch: true, last_seen_at: null },
			{ id: 'u-bob', name: 'bob', created_at: '', revoked_at: null, disabled_at: null, can_dispatch: true, last_seen_at: null }
		]),
		useAllMachines: gate(() => [
			{ id: 'm-bob', user_id: 'u-bob', name: 'bench', display_name: null, revoked_at: null, liveness: 'online' }
		]),
		useHiddenOwners: gate(() => ({ owners: state.owners })),
		useHiddenOwnerActions: () => actions
	};
});

import SessionVisibilityGroup from './SessionVisibilityGroup.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	state.owners = [];
	state.enabled = [];
	vi.clearAllMocks();
});

function render(isAdmin = true) {
	comp = mount(SessionVisibilityGroup, { target: document.body, props: { isAdmin } });
	flushSync();
}

const sw = (kind: 'user' | 'machine', id: string) =>
	document.querySelector<HTMLElement>(`[data-visibility="${kind}"][data-target="${id}"]`);

describe('Instance › Users & access › Show sessions in my list', () => {
	it('is not rendered for a non-admin, and never enables its queries', () => {
		render(false);
		expect(document.querySelector('[data-visibility]')).toBeNull();
		expect(state.enabled.every((enabled) => !enabled())).toBe(true);
	});

	it('shows one switch per other user and per machine, ON by default', () => {
		render();
		expect(sw('user', 'u-me')).toBeNull();
		expect(sw('user', 'u-bob')?.getAttribute('aria-checked')).toBe('true');
		expect(sw('machine', 'm-bob')?.getAttribute('aria-checked')).toBe('true');
		expect(document.querySelector('[data-journey="hidden-chip"]')).toBeNull();
	});

	it('turning a user OFF writes them to the admin hidden list', async () => {
		render();
		sw('user', 'u-bob')?.click();
		await tick();
		expect(actions.hide).toHaveBeenCalledWith('user', 'u-bob', 'bob');
		expect(actions.unhide).not.toHaveBeenCalled();
	});

	it('a hidden user reads OFF with a hidden chip, and turning it ON shows them again', async () => {
		state.owners = [{ kind: 'user', target_id: 'u-bob', label: 'bob' }];
		render();
		expect(sw('user', 'u-bob')?.getAttribute('aria-checked')).toBe('false');
		expect(document.querySelector('[data-journey="visibility-user"] [data-journey="hidden-chip"]')).not.toBeNull();
		sw('user', 'u-bob')?.click();
		await tick();
		expect(actions.unhide).toHaveBeenCalledWith('user', 'u-bob');
	});

	it('switches a single machine off', async () => {
		render();
		sw('machine', 'm-bob')?.click();
		await tick();
		expect(actions.hide).toHaveBeenCalledWith('machine', 'm-bob', 'bench');
	});
});
