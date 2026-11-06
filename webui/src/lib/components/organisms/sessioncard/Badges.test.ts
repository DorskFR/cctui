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

function render(over: Partial<SessionListItem>, extra: { compact?: boolean; unreadCount?: number } = {}) {
	const s = { id: 's1', labels: [], working_dir: '/w', auto_approve: false, ...over } as SessionListItem;
	const view = { s, pendingCount: 0, unreadCount: extra.unreadCount ?? 0 } as SessionView;
	comp = mount(Badges, { target: document.body, props: { view, compact: extra.compact } });
	return document.querySelector<HTMLElement>('[data-journey="card-context"]');
}

describe('session card context chip', () => {
	it('shows the rule and memory total, spelled out on hover', () => {
		const chip = render({ context_counts: { rules: 2, memories: 3 } });
		expect(chip).not.toBeNull();
		expect(chip!.textContent?.trim()).toBe('5');
		expect(chip!.getAttribute('title')).toBe(m.conversation_context_chip({ rules: 2, memories: 3 }));
	});

	it('is absent when the session carries no context', () => {
		expect(render({})).toBeNull();
	});

	it('is absent when both counts are zero', () => {
		expect(render({ context_counts: { rules: 0, memories: 0 } })).toBeNull();
	});
});

describe('compact row badges', () => {
	it('leaves the context chip to the card and the drawer', () => {
		expect(render({ context_counts: { rules: 2, memories: 3 } }, { compact: true })).toBeNull();
	});

	it('names the unread count instead of painting it in the danger tone', () => {
		render({}, { unreadCount: 3 });
		const chip = document.querySelector(`[aria-label="${m.sessions_unread_title({ count: 3 })}"]`);
		expect(chip?.textContent?.trim()).toBe('3');
		expect(chip?.classList.contains('unread')).toBe(true);
		expect(document.querySelector('[data-tone="danger"]')).toBeNull();
	});

	it('spells auto-approve out', () => {
		render({ auto_approve: true });
		expect(document.body.textContent).toContain(m.list_auto_approve());
		expect(document.body.textContent).not.toContain('⚡');
	});
});
