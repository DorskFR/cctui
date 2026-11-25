// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { UserRow } from '@bindings/UserRow';

vi.mock('#lib/features.svelte.ts', () => ({ hasFeature: () => true }));

import AccessUserList from './AccessUserList.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const user = (id: string, revoked = false): UserRow => ({
	id,
	name: id,
	created_at: '',
	revoked_at: revoked ? '2026-01-01' : null,
	disabled_at: null,
	can_dispatch: true,
	last_seen_at: null
});

function render(users: UserRow[], presence: (u: UserRow) => string) {
	comp = mount(AccessUserList, {
		target: document.body,
		props: { users, selectedId: '', meta: () => '2 machines', presence, onselect: () => {}, oncreate: () => {} }
	});
	flushSync();
}
const metaOf = (name: string) =>
	[...document.body.querySelectorAll('[data-journey="user"]')]
		.find((b) => b.getAttribute('data-journey-key') === name)
		?.querySelector('.mt')
		?.textContent?.replace(/\s+/g, ' ')
		.trim();

describe('AccessUserList', () => {
	it('writes machine liveness in the muted line instead of a dot', () => {
		render([user('ana'), user('bo')], (u) => (u.id === 'ana' ? '1 machine online' : 'offline'));
		expect(metaOf('ana')).toBe('2 machines · 1 machine online');
		expect(metaOf('bo')).toBe('2 machines · offline');
		expect(document.body.querySelector('[data-tsu="Dot"]')).toBeNull();
	});

	it('adds nothing when there is no presence to report', () => {
		render([user('cy')], () => '');
		expect(metaOf('cy')).toBe('2 machines');
	});
});
