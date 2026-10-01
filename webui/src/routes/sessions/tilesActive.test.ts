import { describe, expect, it } from 'vitest';
import tiles from './SessionTiles.svelte?raw';
import pane from '../../lib/components/organisms/ConversationPane.svelte?raw';
import header from '../../lib/components/organisms/conversation/DrawerHeader.svelte?raw';

describe('the tiles grid owns which pane holds the keyboard', () => {
	it('keeps exactly one active tile, moved by click or focus', () => {
		expect(tiles).toContain('onpointerdown={() => (picked = s.id)}');
		expect(tiles).toContain('onfocusin={() => (picked = s.id)}');
		expect(tiles).toContain('active={active === s.id}');
		expect(tiles).toContain('class:active={active === s.id}');
	});

	it('highlights the active tile itself', () => {
		expect(tiles.slice(tiles.indexOf('<style>'))).toMatch(/\.tile\.active \{[^}]*outline:/);
	});

	it('gates the header window handler on that one pane', () => {
		expect(pane).toContain('shortcuts={active}');
		expect(header).toContain('active: shortcuts');
		expect(header).not.toContain('isFindChord');
		expect(header).not.toContain('isArchiveChord');
	});

	it('interrupts rather than closes when a tile takes Escape', () => {
		expect(pane).toContain(
			"onescapeaction={chrome === 'tile' && stream.working ? sa.interrupt : undefined}"
		);
		expect(header).toContain('(onescapeaction ?? onclose)?.()');
	});
});
