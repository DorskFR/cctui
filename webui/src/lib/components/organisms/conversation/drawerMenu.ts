import type { MenuItem } from '@dorsk/tsumikit';
import { m } from '$lib/paraglide/messages';

export type MenuGroupId = 'share' | 'branch' | 'collaborate' | 'session';

export interface MenuGroup {
	id: MenuGroupId;
	items: MenuItem[];
}

export function groupedMenu(groups: MenuGroup[], separator: MenuItem['control']): MenuItem[] {
	const out: MenuItem[] = [];
	for (const g of groups) {
		if (g.items.length === 0) continue;
		if (out.length > 0) {
			out.push({
				id: `sep-${g.id}`,
				label: '',
				control: separator,
				attrs: { 'data-menu-sep': g.id, style: 'padding-block: 0' } as MenuItem['attrs']
			});
		}
		out.push(...g.items);
	}
	return out;
}

export function branchOrder<T>(fork: T, followup: T | null, preferFollowup: boolean): T[] {
	if (!followup) return [fork];
	return preferFollowup ? [followup, fork] : [fork, followup];
}

export interface DrawerMenuState {
	archived: boolean;
	compact: boolean;
	collapsed: boolean;
	renaming: boolean;
	keepalive: boolean;
	preferFollowup: boolean;
	cctuiverse: boolean;
	roomName: string | null;
	forkSelectActive?: boolean;
	textSize?: MenuItem['control'];
}

export interface DrawerMenuActions {
	copylink: () => void;
	copymarkdown: () => void;
	export: () => void;
	fork: () => void;
	forkselect?: () => void;
	followup?: () => void;
	search?: () => void;
	room?: () => void;
	invite: () => void;
	join: () => void;
	rename: () => void;
	save: () => void;
	keepalive: () => void;
	archive: () => void;
}

export function drawerMenuGroups(s: DrawerMenuState, a: DrawerMenuActions): MenuGroup[] {
	const share: MenuItem[] = [
		{
			label: m.drawer_copy_link_label(),
			icon: 'link',
			attrs: { title: m.drawer_copy_link_title() },
			onselect: a.copylink
		},
		{
			label: m.drawer_copy_markdown_label(),
			icon: 'markdown',
			attrs: { title: m.drawer_copy_markdown_title() },
			onselect: a.copymarkdown
		},
		{
			label: m.drawer_export_label(),
			icon: 'download',
			attrs: { title: m.drawer_export_title() },
			onselect: a.export
		}
	];

	const branch = branchOrder<MenuItem>(
		{
			label: m.drawer_fork_label(),
			icon: 'fork',
			pressed: a.forkselect ? !!s.forkSelectActive : undefined,
			attrs: { title: a.forkselect ? m.drawer_fork_select_title() : m.drawer_fork_title() },
			onselect: a.forkselect ?? a.fork
		},
		a.followup ? { label: m.drawer_followup_label(), icon: 'arrow-right', onselect: a.followup } : null,
		s.preferFollowup
	);

	const collaborate: MenuItem[] = [];
	if (a.room)
		collaborate.push({
			label: m.rooms_menu_action(),
			icon: 'grid',
			attrs: { title: s.roomName ?? m.rooms_menu_label() },
			onselect: a.room
		});
	if (s.cctuiverse && !s.archived)
		collaborate.push(
			{ label: m.cctuiverse_menu_invite(), icon: 'link', onselect: a.invite },
			{ label: m.cctuiverse_menu_join(), icon: 'link', onselect: a.join }
		);

	const session: MenuItem[] = [];
	if (s.collapsed && a.search)
		session.push({
			label: m.conversation_search_label(),
			icon: 'search',
			attrs: { title: m.conversation_search_title() },
			onselect: a.search
		});
	session.push(
		s.renaming
			? { label: m.common_save(), icon: 'check', onselect: a.save }
			: { label: m.drawer_rename(), icon: 'edit', onselect: a.rename },
		{ label: m.drawer_keepalive_label(), icon: 'recycle', pressed: s.keepalive, onselect: a.keepalive }
	);
	if (!s.compact && s.textSize) session.push({ id: 'text-size', label: m.chrome_text_size(), control: s.textSize });
	if (!s.archived) session.push({ label: m.drawer_archive(), icon: 'archive', onselect: a.archive });

	return [
		{ id: 'share', items: share },
		{ id: 'branch', items: branch },
		{ id: 'collaborate', items: collaborate },
		{ id: 'session', items: session }
	];
}
