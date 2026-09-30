import { describe, expect, it } from 'vitest';
import header from './DrawerHeader.svelte?raw';
import toolbar from './DrawerToolbar.svelte?raw';
import pane from '../ConversationPane.svelte?raw';

const items = () => {
	const start = header.indexOf('const overflowItems');
	expect(start).toBeGreaterThan(-1);
	return header.slice(start, header.indexOf(']);', start));
};

const markup = () => header.slice(header.indexOf('</script>'));

describe('drawer header ⋯ menu', () => {
	it('renders its own always-present menu, not the collapse-only kit overflow', () => {
		expect(markup()).toContain('<Menu label={m.drawer_more_actions()} items={overflowItems}');
		expect(markup()).not.toMatch(/<Toolbar[^>]*\bitems=/);
		const menuAt = markup().indexOf('<Menu');
		const lastIf = markup().lastIndexOf('{#if', menuAt);
		const lastEnd = markup().lastIndexOf('{/if}', menuAt);
		expect(lastEnd).toBeGreaterThan(lastIf);
	});

	it('holds copy link and fork permanently, and no terminal entry', () => {
		const list = items();
		expect(list).toContain('m.drawer_copy_link_label()');
		expect(list).toContain('m.drawer_fork_label()');
		expect(list).toContain('pressed: onforkselect ? forkSelectActive : undefined');
		expect(list).not.toContain('m.drawer_terminal_label()');
		expect(list).not.toContain('terminal');
		expect(header).not.toContain('onterminal');
	});

	it('only stands in for rename while the bar is collapsed', () => {
		expect(items()).toMatch(/\.\.\.\(collapsed\s*\?\s*\[\s*renaming/);
		expect(header).toContain('collapseBelow="{COLLAPSE_BELOW}px"');
	});

	it('does not render the menu actions inline', () => {
		for (const icon of ['link', 'markdown', 'download', 'fork']) {
			expect(markup(), icon).not.toMatch(new RegExp(`<IconButton[^>]*icon="${icon}"`));
		}
	});

	it('gives every entry an icon, checkable ones included', () => {
		const entries = items()
			.split(/\blabel:/)
			.slice(1);
		expect(entries.length).toBeGreaterThanOrEqual(6);
		for (const e of entries) expect(e, e.slice(0, 40)).toMatch(/\bicon:/);
		expect(items()).toContain("icon: 'recycle' as const");
		expect(header).toMatch(/followupItem = \$derived<MenuItem \| null>\([\s\S]*?icon: 'arrow-right'/);
	});

	it('anchors the tour on the menu trigger', () => {
		expect(markup()).toContain('data-journey="actions"');
		expect(markup()).not.toContain('data-journey="fork"');
	});

	it('wires the terminal to the toolbar, not the header', () => {
		const head = pane.slice(pane.indexOf('<DrawerHeader'), pane.indexOf('/>', pane.indexOf('<DrawerHeader')));
		const bar = pane.slice(pane.indexOf('<DrawerToolbar'), pane.indexOf('/>', pane.indexOf('<DrawerToolbar')));
		expect(head).not.toContain('terminal');
		expect(bar).toContain('{terminalOpen}');
		expect(bar).toContain('ontoggleTerminal=');
		expect(toolbar).toContain('data-journey="terminal"');
	});

	it('offers find-in-conversation inline and in the collapsed menu, never both', () => {
		expect(markup()).toMatch(/<IconButton\s+data-overflow[\s\S]{0,200}icon="search"/);
		const collapsedOnly = items().slice(0, items().indexOf('m.drawer_copy_link_label'));
		expect(collapsedOnly).toContain('m.conversation_search_label()');
		expect(items().slice(collapsedOnly.length)).not.toContain('m.conversation_search_label()');
	});

	it('binds ⌘F / Ctrl+F to the find bar and gives it first refusal on Escape', () => {
		expect(header).toContain('isFindChord(e)');
		expect(header).toMatch(/if \(onescape\?\.\(\)\) \{/);
		const esc = header.indexOf("e.key !== 'Escape'");
		const close = header.indexOf('onclose?.()', esc);
		expect(close).toBeGreaterThan(-1);
		expect(header.indexOf('onescape?.()', esc)).toBeLessThan(close);
	});

	it('adds no :global override', () => {
		expect(header).not.toContain(':global(');
	});
});

describe('linked-issue entries', () => {
	it('offers Link issue… when nothing is linked and Change/Unlink when one is', () => {
		const src = items();
		expect(src).toContain('m.plugin_issue_menu_link()');
		expect(src).toContain('m.plugin_issue_menu_change()');
		expect(src).toContain('m.plugin_issue_menu_unlink()');
		expect(src).toContain('issueLinked ?');
	});

	it('labels the entry with an issue glyph, never the labels tag', () => {
		const src = items();
		const entry = src.slice(src.indexOf('plugin_issue_menu_link'));
		expect(entry).toContain("icon: 'bookmark' as const");
		expect(src).not.toContain("icon: 'tag' as const");
	});

	it('opens the modal rather than an inline editor, and unlinks in place', () => {
		const src = items();
		expect(src).toContain('issueLinkOpen = true');
		expect(src).toContain('setPluginSlot(session.id, YOUTRACK_PLUGIN_ID, null)');
		expect(markup()).toContain('<IssueLinkModal');
	});

	it('hands the detected id to the chip row and the modal', () => {
		expect(markup()).toContain('{detectedIssue}');
		expect(markup()).toContain('detected={detectedIssue}');
	});
});
