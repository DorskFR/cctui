import { describe, expect, it, vi } from 'vitest';
import type { MenuItem } from '@dorsk/tsumikit';
import { branchOrder, drawerMenuGroups, groupedMenu } from './drawerMenu';
import { m } from '$lib/paraglide/messages';

const item = (label: string): MenuItem => ({ label, onselect: () => {} });
const sep = (() => {}) as unknown as MenuItem['control'];

describe('groupedMenu', () => {
	it('puts one separator between consecutive groups, none at the edges', () => {
		const out = groupedMenu(
			[
				{ id: 'share', items: [item('Copy link'), item('Export')] },
				{ id: 'branch', items: [item('Fork')] },
				{ id: 'session', items: [item('Rename')] }
			],
			sep
		);
		expect(out.map((i) => i.label || `|${i.id}`)).toEqual([
			'Copy link',
			'Export',
			'|sep-branch',
			'Fork',
			'|sep-session',
			'Rename'
		]);
		expect(out[0].control).toBeUndefined();
		expect(out.at(-1)?.control).toBeUndefined();
	});

	it('skips empty groups without leaving a double separator', () => {
		const out = groupedMenu(
			[
				{ id: 'share', items: [item('Copy link')] },
				{ id: 'collaborate', items: [] },
				{ id: 'session', items: [item('Rename')] }
			],
			sep
		);
		expect(out.filter((i) => i.control)).toHaveLength(1);
		expect(out.map((i) => i.label)).toEqual(['Copy link', '', 'Rename']);
	});

	it('makes separator rows inert controls with no action', () => {
		const out = groupedMenu(
			[
				{ id: 'share', items: [item('a')] },
				{ id: 'branch', items: [item('b')] }
			],
			sep
		);
		const s = out[1];
		expect(s.control).toBe(sep);
		expect(s.onselect).toBeUndefined();
		expect((s.attrs as Record<string, unknown>)['data-menu-sep']).toBe('branch');
	});

	it('returns nothing for no items', () => {
		expect(groupedMenu([{ id: 'share', items: [] }], sep)).toEqual([]);
	});
});

describe('branchOrder', () => {
	it('lists the preferred action first', () => {
		expect(branchOrder('fork', 'followup', true)).toEqual(['followup', 'fork']);
		expect(branchOrder('fork', 'followup', false)).toEqual(['fork', 'followup']);
	});

	it('drops a missing follow-up', () => {
		expect(branchOrder('fork', null, true)).toEqual(['fork']);
	});
});

describe('drawerMenuGroups', () => {
	const noop = () => {};
	const actions = {
		copylink: noop,
		copymarkdown: noop,
		export: noop,
		fork: noop,
		invite: noop,
		join: noop,
		rename: noop,
		save: noop,
		keepalive: noop,
		archive: noop
	};
	const state = {
		archived: false,
		compact: false,
		collapsed: false,
		renaming: false,
		keepalive: false,
		preferFollowup: false,
		cctuiverse: false,
		roomName: null,
		textSize: sep
	};
	const labels = (g: { items: MenuItem[] }) => g.items.map((i) => i.label);
	const byId = (groups: ReturnType<typeof drawerMenuGroups>, id: string) =>
		groups.find((g) => g.id === id) as { items: MenuItem[] };

	it('orders the groups Share · Branch · Collaborate · Session', () => {
		expect(drawerMenuGroups(state, actions).map((g) => g.id)).toEqual(['share', 'branch', 'collaborate', 'session']);
	});

	it('shares by link, markdown and export', () => {
		expect(labels(byId(drawerMenuGroups(state, actions), 'share'))).toEqual([
			m.drawer_copy_link_label(),
			m.drawer_copy_markdown_label(),
			m.drawer_export_label()
		]);
	});

	it('branches by fork and follow-up, preferred first, fork checkable only with a partial fork', () => {
		const followup = vi.fn();
		const g = byId(drawerMenuGroups({ ...state, preferFollowup: true }, { ...actions, followup }), 'branch');
		expect(labels(g)).toEqual([m.drawer_followup_label(), m.drawer_fork_label()]);
		expect(g.items[1].pressed).toBeUndefined();
		const sel = byId(
			drawerMenuGroups({ ...state, forkSelectActive: true }, { ...actions, forkselect: noop }),
			'branch'
		);
		expect(sel.items[0].pressed).toBe(true);
	});

	it('collaborates by room, invite and join, the cctuiverse pair only when enabled and live', () => {
		const room = vi.fn();
		const on = byId(drawerMenuGroups({ ...state, cctuiverse: true }, { ...actions, room }), 'collaborate');
		expect(labels(on)).toEqual([m.rooms_menu_action(), m.cctuiverse_menu_invite(), m.cctuiverse_menu_join()]);
		on.items[0].onselect?.();
		expect(room).toHaveBeenCalledOnce();
		const archived = byId(drawerMenuGroups({ ...state, cctuiverse: true, archived: true }, actions), 'collaborate');
		expect(archived.items).toHaveLength(0);
	});

	it('keeps rename, cache, text size and archive under Session, never interrupt', () => {
		const g = byId(drawerMenuGroups(state, actions), 'session');
		expect(labels(g)).toEqual([
			m.drawer_rename(),
			m.drawer_keepalive_label(),
			m.chrome_text_size(),
			m.drawer_archive()
		]);
		expect(g.items.find((i) => i.id === 'text-size')?.control).toBe(sep);
		expect(labels(g)).not.toContain(m.drawer_interrupt_label());
	});

	it('swaps rename for save while renaming, and drops archive once archived', () => {
		const g = byId(drawerMenuGroups({ ...state, renaming: true, archived: true }, actions), 'session');
		expect(labels(g)).toEqual([m.common_save(), m.drawer_keepalive_label(), m.chrome_text_size()]);
	});

	it('stands in for search only while the bar is collapsed, and drops text size in a tile', () => {
		const search = vi.fn();
		const wide = byId(drawerMenuGroups(state, { ...actions, search }), 'session');
		expect(labels(wide)).not.toContain(m.conversation_search_label());
		const tile = byId(drawerMenuGroups({ ...state, collapsed: true, compact: true }, { ...actions, search }), 'session');
		expect(labels(tile)[0]).toBe(m.conversation_search_label());
		expect(labels(tile)).not.toContain(m.chrome_text_size());
	});

	it('gives every action an icon', () => {
		const groups = drawerMenuGroups({ ...state, cctuiverse: true }, { ...actions, room: noop, followup: noop });
		for (const item of groups.flatMap((g) => g.items)) {
			if (!item.control) expect(item.icon, item.label).toBeTruthy();
		}
	});
});
