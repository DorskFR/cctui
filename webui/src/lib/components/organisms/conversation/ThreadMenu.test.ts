// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, tick, unmount } from 'svelte';
import type { SessionListItem } from '@bindings/SessionListItem';
import type { ThreadSession } from '@bindings/ThreadSession';
import { threadEntries } from './thread';

const query = vi.hoisted(() => ({ data: undefined as { sessions: ThreadSession[] } | undefined }));
const goto = vi.hoisted(() => vi.fn());
vi.mock('$lib/queries', () => ({ useSessionThread: () => query }));
vi.mock('$app/navigation', () => ({ goto }));
vi.mock('$lib/paraglide/messages', () => ({
	m: {
		drawer_thread_menu: () => 'Sessions in this thread',
		drawer_thread_previous: () => 'Previous',
		drawer_thread_next: () => 'Next'
	}
}));

import ThreadMenu from './ThreadMenu.svelte';

const FALLBACK = { previous: 'Previous', next: 'Next' };
const t = (id: string, name: string | null, at: string): ThreadSession => ({
	id,
	name,
	working_dir: '/w',
	registered_at: at
});
const THREAD = [
	t('a', 'first', '2026-10-18T10:00:00Z'),
	t('b', 'second', '2026-10-19T10:00:00Z'),
	t('c', null, '2026-10-20T10:00:00Z')
];

function session(over: Partial<SessionListItem> = {}): SessionListItem {
	return {
		id: 'b',
		name: 'second',
		working_dir: '/w',
		parent_id: null,
		metadata: null,
		next_session_id: null,
		registered_at: '2026-10-19T10:00:00Z',
		status: 'archived',
		...over
	} as SessionListItem;
}

describe('threadEntries', () => {
	it('lists the fetched thread oldest first with the current one marked', () => {
		const e = threadEntries(session(), THREAD, FALLBACK);
		expect(e.map((x) => x.id)).toEqual(['a', 'b', 'c']);
		expect(e.map((x) => x.current)).toEqual([false, true, false]);
		expect(e[2].title).toBe('/w');
	});

	it('falls back to the session links until the thread lands', () => {
		const s = session({ parent_id: 'a', metadata: { relation: 'followup' }, next_session_id: 'c' });
		const e = threadEntries(s, undefined, FALLBACK);
		expect(e.map((x) => [x.id, x.title, x.current])).toEqual([
			['a', 'Previous', false],
			['b', 'second', true],
			['c', 'Next', false]
		]);
	});

	it('keeps an archived session in the thread its stale row has no next for', () => {
		const e = threadEntries(session({ next_session_id: null }), THREAD, FALLBACK);
		expect(e.map((x) => x.id)).toEqual(['a', 'b', 'c']);
	});

	it('is empty for a session with no previous and no next', () => {
		expect(threadEntries(session(), undefined, FALLBACK)).toEqual([]);
		expect(threadEntries(session(), [t('b', 'second', 'x')], FALLBACK)).toEqual([]);
		const fork = session({ parent_id: 'a', metadata: { relation: 'fork' } });
		expect(threadEntries(fork, undefined, FALLBACK)).toEqual([]);
	});

	it('gives unnamed sessions of one directory distinct labels', () => {
		const e = threadEntries(
			session({ id: 'x', name: null }),
			[t('x', null, '1'), t('y', null, '2'), t('z', null, '3')],
			FALLBACK
		);
		expect(new Set(e.map((x) => x.label)).size).toBe(3);
	});
});

describe('ThreadMenu', () => {
	let comp: ReturnType<typeof mount> | null = null;

	beforeEach(() => {
		query.data = undefined;
		goto.mockReset();
	});
	afterEach(() => {
		if (comp) unmount(comp);
		comp = null;
		document.body.innerHTML = '';
	});

	function render(s: SessionListItem) {
		comp = mount(ThreadMenu, { target: document.body, props: { session: s } });
		flushSync();
	}

	// happy-dom has no Popover API: open the panel by hand.
	async function openMenu() {
		const panel = document.querySelector('[popover]') as
			| (HTMLElement & { hidePopover?: () => void })
			| null;
		expect(panel).not.toBeNull();
		if (panel && typeof panel.hidePopover !== 'function') panel.hidePopover = () => {};
		const toggle = new Event('toggle') as Event & { newState: string };
		toggle.newState = 'open';
		panel?.dispatchEvent(toggle);
		await tick();
		return [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')];
	}

	it('is hidden when the session is alone', () => {
		query.data = { sessions: [t('b', 'second', 'x')] };
		render(session());
		expect(document.querySelector('[data-journey="head-thread"]')).toBeNull();
	});

	it('lists every session in order, the current one bold, and navigates on pick', async () => {
		query.data = { sessions: THREAD };
		render(session());
		const rows = await openMenu();
		expect(rows.map((r) => r.textContent?.trim().split(/\s+/)[0])).toEqual(['first', 'second', '/w']);
		expect(rows.map((r) => r.getAttribute('aria-current'))).toEqual([null, 'page', null]);
		expect(rows[1].querySelector('.fw-bold')).not.toBeNull();
		expect(rows[0].querySelector('.fw-bold')).toBeNull();

		rows[2].click();
		flushSync();
		expect(goto).toHaveBeenCalledWith('/sessions/c');
	});

	it('does not navigate when the current session is picked', async () => {
		query.data = { sessions: THREAD };
		render(session());
		const rows = await openMenu();
		rows[1].click();
		flushSync();
		expect(goto).not.toHaveBeenCalled();
	});
});
