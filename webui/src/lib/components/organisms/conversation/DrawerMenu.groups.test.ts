import { describe, expect, it } from 'vitest';
import menu from './DrawerMenu.svelte?raw';
import en from '../../../../../messages/en.json?raw';
import fr from '../../../../../messages/fr.json?raw';

const items = menu.slice(menu.indexOf('const overflowItems'), menu.indexOf(']);', menu.indexOf('const overflowItems')));
const at = (s: string) => {
	const i = items.indexOf(s);
	expect(i, s).toBeGreaterThan(-1);
	return i;
};

describe('drawer ⋯ menu groups', () => {
	it('runs Share · Branch · Collaborate · Session in that order', () => {
		const groups = ['share', 'branch', 'collaborate', 'session'].map((g) => at(`group(m.drawer_menu_group_${g}()`));
		expect([...groups].sort((a, b) => a - b)).toEqual(groups);
	});

	it('files each action under its group', () => {
		const order = [
			'group(m.drawer_menu_group_share()',
			'm.drawer_copy_link_label()',
			'm.drawer_copy_markdown_label()',
			'm.drawer_export_label()',
			'group(m.drawer_menu_group_branch()',
			'm.drawer_fork_label()',
			'group(m.drawer_menu_group_collaborate()',
			'm.rooms_menu_action()',
			'm.cctuiverse_menu_invite()',
			'm.cctuiverse_menu_join()',
			'group(m.drawer_menu_group_session()',
			'm.drawer_rename()',
			'm.drawer_keepalive_label()',
			'm.drawer_archive()'
		].map(at);
		expect([...order].sort((a, b) => a - b)).toEqual(order);
	});

	it('heads a group with a non-focusable row and drops an empty group', () => {
		expect(menu).toContain('label, control: groupHeading }, ...items] : [];');
		expect(menu).toContain('return items.length ?');
		const heading = menu.slice(menu.indexOf('{#snippet groupHeading'), menu.indexOf('{/snippet}', menu.indexOf('{#snippet groupHeading')));
		expect(heading).not.toMatch(/<button|tabindex/);
	});

	it('translates every group heading', () => {
		for (const msgs of [en, fr]) {
			const json = JSON.parse(msgs);
			for (const g of ['share', 'branch', 'collaborate', 'session']) expect(json[`drawer_menu_group_${g}`]).toBeTruthy();
		}
	});
});
