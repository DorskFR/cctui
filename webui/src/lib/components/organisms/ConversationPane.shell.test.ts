import { describe, expect, it } from 'vitest';
import drawer from './ConversationDrawer.svelte?raw';
import pane from './ConversationPane.svelte?raw';
import composer from './conversation/ConversationComposer.svelte?raw';
import header from './conversation/DrawerHeader.svelte?raw';
import forkbar from './conversation/ForkSelectBar.svelte?raw';
import overlays from './conversation/PaneOverlays.svelte?raw';
import model from './conversation/paneModel.svelte.ts?raw';

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

describe('ConversationPane chrome', () => {
	it('is positioned so nothing inside it centres on the viewport', () => {
		const css = pane.slice(pane.indexOf('<style>'));
		expect(css).toMatch(/\.conv-pane \{[^}]*position: relative;/);
		expect(forkbar).toContain('.fork-select-bar.contained');
		expect(forkbar).toContain('position: absolute');
		expect(pane).toContain('<PaneOverlays {p} {canFork} contained={tile} />');
		expect(overlays).toContain('<ForkSelectBar\n\t\t{contained}');
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

	it('holds the session open for the notifier while it is mounted', () => {
		expect(model).toContain('notify.holdOpen(this.id)');
	});

	it('remounts the composer per session so a switch never writes one draft over another', () => {
		const markup = pane.slice(pane.indexOf('</script>'));
		const at = markup.indexOf('<ConversationComposer');
		const open = markup.lastIndexOf('{#key', at);
		expect(open).toBeGreaterThan(-1);
		expect(markup.slice(open, at).trim()).toBe('{#key session.id}');
		expect(markup.indexOf('{/key}', at)).toBeGreaterThan(at);
		expect(composer).toContain('let input = $state(drafts.get(composerKey(session.id)));');
	});

	it('adds no :global override', () => {
		expect(pane).not.toContain(':global(');
		expect(drawer).not.toContain(':global(');
	});
});
