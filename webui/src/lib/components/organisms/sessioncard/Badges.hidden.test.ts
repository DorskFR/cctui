// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import { mount, unmount } from 'svelte';
import type { SessionListItem } from '@bindings/SessionListItem';
import { m } from '$lib/paraglide/messages';
import Badges from './Badges.svelte';
import type { SessionView } from './view';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

function chip(over: Partial<SessionListItem>) {
	const s = { id: 's1', labels: [], working_dir: '/w', auto_approve: false, ...over } as SessionListItem;
	comp = mount(Badges, { target: document.body, props: { view: { s, pendingCount: 0, unreadCount: 0 } as SessionView } });
	return document.querySelector<HTMLElement>('[data-journey="hidden-chip"]');
}

describe('session hidden chip', () => {
	it('marks a session the server flagged hidden', () => {
		const el = chip({ hidden: true });
		expect(el?.textContent?.trim()).toBe(m.hidden_owners_chip());
		expect(el?.getAttribute('title')).toBe(m.hidden_owners_chip_title());
	});

	it('is absent on an ordinary session', () => {
		expect(chip({})).toBeNull();
		expect(chip({ hidden: false })).toBeNull();
	});
});
