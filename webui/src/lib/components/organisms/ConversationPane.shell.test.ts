import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import drawer from './ConversationDrawer.svelte?raw';
import pane from './ConversationPane.svelte?raw';
import header from './conversation/DrawerHeader.svelte?raw';
import forkbar from './conversation/ForkSelectBar.svelte?raw';
import dock from './ConversationDock.svelte?raw';
import overlays from './conversation/PaneOverlays.svelte?raw';

describe('ConversationDrawer is only the panel', () => {
	it('delegates every conversation internal to the pane', () => {
		expect(drawer).toContain('<ConversationPane');
		expect(drawer).toContain('chrome="drawer"');
		for (const gone of [
			'ConversationStream',
			'DrawerToolbar',
			'ConversationComposer',
			'ForkModal',
			'MessagePins'
		]) {
			expect(drawer, gone).not.toContain(gone);
		}
	});

	it('keeps the panel geometry and the scroll lock in the drawer', () => {
		expect(drawer).toContain('<ResizablePanel');
		expect(drawer).toContain('fullWidthBelow="959px"');
		expect(drawer).toContain('widthKey="cctui_drawer_width"');
		expect(drawer).toContain('lockDocumentScroll()');
		expect(pane, 'a tile must not lock the page scroll').not.toContain('lockDocumentScroll');
		expect(pane).not.toContain('ResizablePanel');
	});
});

const kitPanel = readFileSync('node_modules/@dorsk/tsumikit/dist/components/layouts/ResizablePanel.svelte', 'utf8');

describe('ConversationDrawer scrim', () => {
	it('dims the page at 40% behind the overlay drawer, whatever the theme', () => {
		expect(drawer).toContain('scrim={false}');
		const scrim = drawer.slice(drawer.indexOf('<Scrim'), drawer.indexOf('/>', drawer.indexOf('<Scrim')));
		expect(scrim).toContain('{onclose}');
		expect(scrim).toContain('hideBelow="959px"');
		expect(scrim).toContain('background: var(--chrome-scrim)');
		expect(drawer).toContain('--chrome-scrim: color-mix(in srgb, black 40%, transparent)');
		expect(drawer.indexOf('<Scrim')).toBeLessThan(drawer.indexOf('<ResizablePanel'));
	});

	it('sits under the panel: the kit isolates the panel, so the layers are set here', () => {
		expect(kitPanel).toMatch(/\.panel-layout \{[^}]*isolation: isolate;/);
		const scrim = drawer.slice(drawer.indexOf('<Scrim'), drawer.indexOf('/>', drawer.indexOf('<Scrim')));
		expect(scrim).toContain('z={0}');
		const panel = drawer.slice(drawer.indexOf('<ResizablePanel'), drawer.indexOf('>', drawer.indexOf('<ResizablePanel')));
		expect(panel).toContain('style="z-index: 1"');
		expect(drawer.indexOf('<Scrim')).toBeLessThan(drawer.indexOf('<ResizablePanel'));
	});

	it('is the drawer only: the docked conversation stays a split view', () => {
		expect(dock).not.toContain('Scrim');
	});
});

describe('ConversationPane chrome', () => {
	it('is positioned so nothing inside it centres on the viewport', () => {
		const css = pane.slice(pane.indexOf('<style>'));
		expect(css).toMatch(/\.conv-pane \{[^}]*position: relative;/);
		expect(forkbar).toContain('.fork-select-bar.contained');
		expect(forkbar).toContain('position: absolute');
		expect(pane).toContain("contained={chrome === 'tile'}");
		expect(overlays).toContain('{contained}');
	});

	it('drops the back control and adds maximize when the shell asks for them', () => {
		expect(header).toContain('{#if onclose}');
		expect(header).toContain('{#if onmaximize}');
	});

	it('has no "open in tiles" entry: tiles are a Sessions view mode', () => {
		expect(pane).not.toContain('onopenintiles');
		expect(header).not.toContain('tiles_open_here');
		expect(pane).not.toContain('tilesHref');
	});

	it('feeds the needs-input state to the activity line, not the banners', () => {
		expect(pane).toContain('<ActivityBanner {stream} {archived} {needsInput} since={session.last_activity_at} />');
		expect(pane).toContain('<DrawerBanners sessionId={id} {stream} bind:acctModalOpen />');
	});

	it('holds the session open for the notifier while it is mounted', () => {
		expect(pane).toContain('notify.holdOpen(id)');
	});

	it('adds no :global override', () => {
		expect(pane).not.toContain(':global(');
		expect(drawer).not.toContain(':global(');
	});
});
