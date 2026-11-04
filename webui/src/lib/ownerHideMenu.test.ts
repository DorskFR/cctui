import { afterEach, describe, expect, it, vi } from 'vitest';
import { m } from '$lib/paraglide/messages';
import { toasts } from '$lib/toast.svelte';
import type { OwnerTarget } from './hiddenOwners';
import { ownerTargetLabel, toggleOwnerTarget } from './ownerHideMenu.svelte';
import header from './components/organisms/conversation/DrawerHeader.svelte?raw';
import pane from './components/organisms/ConversationPane.svelte?raw';

const user: OwnerTarget = { kind: 'user', target_id: 'u-bob', label: 'bob', hidden: false };
const machine: OwnerTarget = { kind: 'machine', target_id: 'm1', label: 'workbench', hidden: false };

afterEach(() => vi.restoreAllMocks());

describe('session ⋯ hide entries', () => {
	it('names the user or machine being hidden', () => {
		expect(ownerTargetLabel(user)).toBe(m.hidden_owners_hide_user({ name: 'bob' }));
		expect(ownerTargetLabel(machine)).toBe(m.hidden_owners_hide_machine({ name: 'workbench' }));
	});

	it('flips to the unhide wording once hidden', () => {
		expect(ownerTargetLabel({ ...user, hidden: true })).toBe(m.hidden_owners_unhide_user({ name: 'bob' }));
		expect(ownerTargetLabel({ ...machine, hidden: true })).toBe(
			m.hidden_owners_unhide_machine({ name: 'workbench' })
		);
	});

	it('hides a visible owner and confirms it', async () => {
		const ok = vi.spyOn(toasts, 'ok').mockImplementation(() => undefined as never);
		const actions = { hide: vi.fn(async () => undefined), unhide: vi.fn(async () => undefined) };
		await toggleOwnerTarget(machine, actions);
		expect(actions.hide).toHaveBeenCalledWith('machine', 'm1', 'workbench');
		expect(actions.unhide).not.toHaveBeenCalled();
		expect(ok).toHaveBeenCalledWith(m.hidden_owners_hidden_toast({ name: 'workbench' }));
	});

	it('unhides a hidden owner', async () => {
		vi.spyOn(toasts, 'ok').mockImplementation(() => undefined as never);
		const actions = { hide: vi.fn(async () => undefined), unhide: vi.fn(async () => undefined) };
		await toggleOwnerTarget({ ...user, hidden: true }, actions);
		expect(actions.unhide).toHaveBeenCalledWith('user', 'u-bob');
		expect(actions.hide).not.toHaveBeenCalled();
	});

	it('reports a refusal instead of throwing', async () => {
		const err = vi.spyOn(toasts, 'error').mockImplementation(() => undefined as never);
		const actions = {
			hide: vi.fn(async () => {
				throw new Error('forbidden');
			}),
			unhide: vi.fn(async () => undefined)
		};
		await expect(toggleOwnerTarget(user, actions)).resolves.toBeUndefined();
		expect(err).toHaveBeenCalledWith('forbidden');
	});

	it('is appended to the drawer ⋯ menu from the pane', () => {
		const start = header.indexOf('const overflowItems');
		expect(header.slice(start, header.indexOf(']);', start))).toContain('...ownerItems');
		expect(pane).toContain('ownerItems={ownerMenu.items}');
		expect(pane).toMatch(/useOwnerHideMenu\(\s*\(\) => session,\s*\(\) => me\.data\s*\)/);
	});
});
