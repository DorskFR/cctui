// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import type { PasskeyRow } from '@bindings/PasskeyRow';
import { m } from '#lib/paraglide/messages.js';

const state = vi.hoisted(() => ({
	rows: [] as unknown[],
	revokePasskey: null as unknown as (userId: string, id: string) => Promise<void>
}));

vi.mock('#lib/queries/index.ts', () => ({
	useUserPasskeys: () => ({ data: { passkeys: state.rows }, isLoading: false }),
	useUserActions: () => ({ revokePasskey: state.revokePasskey })
}));
vi.mock('#lib/toast.svelte.ts', () => ({ toasts: { ok: vi.fn(), error: vi.fn() } }));

const { default: AccessPasskeysTab } = await import('./AccessPasskeysTab.svelte');

const passkey = (id: string, label: string, lastUsed: string | null = null): PasskeyRow => ({
	id,
	label,
	discoverable: true,
	created_at: '2026-01-01T00:00:00Z',
	last_used_at: lastUsed
});

const button = (label: string, scope = 'body') =>
	[...document.querySelectorAll<HTMLButtonElement>(`${scope} button`)].find(
		(b) => b.textContent?.trim() === label
	);

let comp: ReturnType<typeof mount> | null = null;
const render = (canManage: boolean) => {
	comp = mount(AccessPasskeysTab, {
		target: document.body,
		props: { userId: 'u1', userName: 'jeanne', canManage }
	});
	flushSync();
};

beforeEach(() => {
	state.rows = [];
	state.revokePasskey = vi.fn(async () => {});
});
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

describe('AccessPasskeysTab', () => {
	it('lists the user’s passkeys and deletes one only after a danger confirm', async () => {
		state.rows = [passkey('p1', 'YubiKey'), passkey('p2', 'iPhone', '2026-02-01T00:00:00Z')];
		render(true);
		const rows = [...document.querySelectorAll('tbody tr[data-part="row"]')];
		expect(rows).toHaveLength(2);
		expect(rows[0].textContent).toContain('YubiKey');
		expect(rows[0].textContent).toContain(m.settings_passkeys_never_used());

		rows[0].querySelector<HTMLButtonElement>('td.dt-actions button')?.click();
		flushSync();
		const dialog = document.querySelector('dialog[open]');
		expect(dialog?.textContent).toContain(
			m.users_confirm_revoke_passkey({ name: 'jeanne', label: 'YubiKey' })
		);
		expect(state.revokePasskey).not.toHaveBeenCalled();

		button(m.settings_passkeys_revoke(), 'dialog[open]')?.click();
		await vi.waitFor(() => expect(state.revokePasskey).toHaveBeenCalledWith('u1', 'p1'));
	});

	it('offers no delete to a viewer who cannot manage', () => {
		state.rows = [passkey('p1', 'YubiKey')];
		render(false);
		expect(document.querySelector('td.dt-actions button')).toBeNull();
	});

	it('shows the empty copy when the user has no passkeys', () => {
		render(true);
		expect(document.querySelector('tr[data-part="empty"]')?.textContent).toContain(
			m.users_passkeys_empty()
		);
	});
});
