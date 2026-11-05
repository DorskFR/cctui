import { describe, expect, it } from 'vitest';
import header from './DrawerHeader.svelte?raw';
import toolbar from './DrawerToolbar.svelte?raw';
import keys from './headerKeys.ts?raw';
import pane from '../ConversationPane.svelte?raw';
import meta from './HeaderMeta.svelte?raw';
import footer from '../sessioncard/Footer.svelte?raw';

const script = header.slice(0, header.indexOf('</script>'));
const markup = header.slice(header.indexOf('</script>'));
const block = (name: string) => {
	const start = script.indexOf(`const ${name}`);
	expect(start, name).toBeGreaterThan(-1);
	return script.slice(start, script.indexOf(');\n', start));
};
const bar = () => markup.slice(markup.indexOf('<Toolbar'), markup.indexOf('</Toolbar>'));

describe('drawer header row', () => {
	it('keeps only back, title, search, stop and ⋯ inline', () => {
		const inline = bar();
		expect(inline).not.toContain('<FontScalePicker');
		expect(inline).not.toMatch(/icon="archive"/);
		expect(inline).not.toMatch(/icon="stop"/);
		expect(inline).not.toMatch(/icon="edit"/);
		expect(inline).toContain('icon="chevron-left"');
		expect(inline).toContain('icon="search"');
		expect(inline).toContain('<Menu label={m.drawer_more_actions()} items={overflowItems}');
	});

	it('offers Stop only while a turn runs, outlined with a spinner and a word', () => {
		expect(script).toContain('const stoppable = $derived(working && !archived)');
		const stop = bar().slice(bar().indexOf('{#if stoppable}'));
		const btn = stop.slice(0, stop.indexOf('{/if}'));
		expect(btn).toContain('<Button');
		expect(btn).not.toContain('variant="primary"');
		expect(btn).not.toContain('tone="danger"');
		expect(btn).toContain('<Spinner');
		expect(btn).toContain('{m.chrome_stop()}');
		expect(btn).toContain('onclick={oninterrupt}');
		expect(pane).toContain('working={stream.working}');
	});

	it('gives the title its own line when the drawer is narrow', () => {
		expect(script).toContain('const titleOwnLine = $derived(collapsed && !compact)');
		const after = markup.slice(markup.indexOf('</Toolbar>'));
		expect(after).toMatch(/\{#if titleOwnLine\}\s*\{@render titleBlock\(\)\}/);
	});

	it('anchors the tours on the same elements', () => {
		expect(markup).toContain('data-journey="header"');
		expect(markup).toContain('data-journey="actions"');
		expect(markup).toContain('data-journey="find"');
	});
});

describe('drawer header ⋯ menu', () => {
	const items = block('overflowItems');

	it('builds the grouped menu and separates the groups', () => {
		expect(items).toContain('groupedMenu(');
		expect(items).toContain('drawerMenuGroups(');
		expect(items).toContain('menuSeparator');
		expect(markup).toContain('{#snippet menuSeparator()}');
		expect(markup).toContain('role="separator"');
	});

	it('hands the menu the text-size row and every action', () => {
		expect(items).toContain('textSize: textSizeRow');
		expect(markup).toMatch(/\{#snippet textSizeRow\(\)\}[\s\S]*?<FontScalePicker/);
		for (const k of ['copylink: oncopylink', 'archive: onarchive', 'rename: startRename', 'forkselect: onforkselect'])
			expect(items).toContain(k);
		expect(items).toContain('room: onsetroom ? () => (roomOpen = true) : undefined');
	});

	it('has no interrupt entry and no hide-owner entry', () => {
		expect(header).not.toContain('ownerItems');
		expect(header).not.toContain('m.drawer_interrupt_label()');
		expect(pane).not.toContain('useOwnerHideMenu');
	});

	it('keeps the inline search collapsible into the menu', () => {
		expect(items).toContain('collapsed,');
		expect(items).toContain('search: onsearch');
		expect(header).toContain('collapseBelow="{COLLAPSE_BELOW}px"');
		expect(bar()).toMatch(/<IconButton\s+data-overflow[\s\S]{0,200}icon="search"/);
	});

	it('wires the terminal to the toolbar, not the header', () => {
		const head = pane.slice(pane.indexOf('<DrawerHeader'), pane.indexOf('/>', pane.indexOf('<DrawerHeader')));
		const tb = pane.slice(pane.indexOf('<DrawerToolbar'), pane.indexOf('/>', pane.indexOf('<DrawerToolbar')));
		expect(head).not.toContain('terminal');
		expect(tb).toContain('{terminalOpen}');
		expect(tb).toContain('ontoggleTerminal=');
		expect(toolbar).toContain("'data-journey': 'terminal'");
	});

	it('binds ⌘F / Ctrl+F to the find bar and gives it first refusal on Escape', () => {
		expect(keys).toContain('isFindChord(e)');
		expect(header).toContain("if (action === 'search')");
		expect(header).toMatch(/if \(onescape\?\.\(\)\) \{/);
		const esc = header.indexOf("action !== 'escape'");
		const close = header.indexOf('onclose)?.()', esc);
		expect(close).toBeGreaterThan(-1);
		expect(header.indexOf('onescape?.()', esc)).toBeLessThan(close);
	});

	it('adds no :global override', () => {
		expect(header).not.toContain(':global(');
	});
});

describe('issue linking', () => {
	it('is gone: no menu entry, link modal, or detected-id chip; labels are the only tagging', () => {
		expect(header).not.toContain('plugin_issue_menu');
		expect(header).not.toContain('IssueLinkModal');
		expect(header).not.toContain('detectedIssue');
		expect(meta).not.toContain('PluginChips');
		expect(footer).not.toContain('PluginChips');
	});
});
