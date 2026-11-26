// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import title from './DrawerTitle.svelte?raw';
import menu from './DrawerMenu.svelte?raw';
import RoomBadge from '#lib/components/molecules/RoomBadge.svelte';

const markup = menu.slice(menu.indexOf('</script>'));
const items = menu.slice(menu.indexOf('const overflowItems'), menu.indexOf(']);', menu.indexOf('const overflowItems')));

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

function render(name: string | null) {
	comp = mount(RoomBadge, { target: document.body, props: { name } });
	flushSync();
}

describe('drawer header room control', () => {
	it('keeps no room trigger in the title row', () => {
		expect(title).not.toContain('<Popover');
		expect(title).not.toContain('RoomMenu');
		expect(title).not.toContain('roomtrigger');
	});

	it('shows the read-only badge next to the title', () => {
		expect(title).toContain('<RoomBadge name={session.room_name} />');
	});

	it('renders nothing when the session is in no room', () => {
		render(null);
		expect(document.body.textContent?.trim()).toBe('');
	});

	it('renders the room name when the session is in one', () => {
		render('wave 23');
		expect(document.body.textContent).toContain('wave 23');
	});

	it('offers the room picker from the ⋯ menu', () => {
		expect(items).toContain('m.rooms_menu_action()');
		expect(items).toContain('onselect: () => (roomOpen = true)');
	});

	it('opens the shared RoomMenu from that entry', () => {
		const modal = markup.slice(markup.indexOf('{#if roomOpen}'));
		expect(modal).toContain('<RoomMenu');
		expect(modal).toContain('current={session.room_id ?? null}');
		expect(modal).toContain('onsetroom?.(session.id, pick)');
	});
});
