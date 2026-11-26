// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import header from './DrawerHeader.svelte?raw';
import menu from './DrawerMenu.svelte?raw';

const markup = header.slice(header.indexOf('</script>'));
const controls = markup
	.split(/(?=<IconButton|<DrawerMenu)/)
	.filter((c) => /^<(IconButton|DrawerMenu)/.test(c))
	.map((c) => c.slice(0, c.indexOf('>') + 1));

const named = (label: string) => {
	const c = controls.find((x) => x.includes(label));
	expect(c, label).toBeTruthy();
	return c as string;
};

describe('drawer header control chrome', () => {
	it('sizes every control from one box per density', () => {
		expect(header).toContain("const box: 'sm' | 'md' = $derived(collapsed ? 'sm' : 'md')");
		for (const c of controls) expect(c, c).toContain('{box}');
		expect(markup).not.toContain("box={collapsed ? 'sm' : 'lg'}");
		expect(markup).not.toMatch(/box="(xs|sm|md|lg)"/);
	});

	it('draws every header control unboxed, the ⋯ trigger included', () => {
		expect(markup).not.toMatch(/\bchip\b/);
		expect(header).not.toContain('CHIP_CHROME');
		for (const c of controls.filter((x) => x.startsWith('<IconButton') && !x.includes('m.drawer_back()'))) {
			expect(c, c).toContain('variant="ghost"');
		}
		expect(menu).toContain('placement="bottom-end" variant="ghost" {box}');
		expect(header).not.toContain('--pop-trigger-');
	});

	it('keeps archive inline, drops the inline rename and search buttons', () => {
		for (const icon of ['edit', 'search']) {
			expect(markup, icon).not.toMatch(new RegExp(`<IconButton[^>]*icon="${icon}"`));
		}
		expect(markup).toContain(
			'<IconButton {box} variant="ghost" tone="warn" icon="archive" label={m.drawer_archive()} onclick={onarchive} />'
		);
		const items = menu.slice(menu.indexOf('const overflowItems'));
		expect(items).toContain("{ label: m.drawer_rename(), icon: 'edit' as const, onselect: onrename }");
	});

	it('renames by clicking the title', async () => {
		const title = (await import('./DrawerTitle.svelte?raw')).default;
		expect(header).toContain('onrename={startRename} />');
		expect(title).toMatch(/<button type="button" class="tname" title=\{m\.drawer_rename\(\)\} onclick=\{onrename\}/);
		expect(title).toContain('<Icon name="edit" size={14} />');
		expect(title).toMatch(/\.tname:hover \.pen/);
	});

	it('offers Stop only while a turn runs, outlined with a spinner', () => {
		expect(header).toContain('working = false');
		expect(header).toContain('const canStop = $derived(working && !archived && !compact)');
		const stop = markup.slice(markup.indexOf('{#if canStop}'), markup.indexOf('{/if}', markup.indexOf('{#if canStop}')));
		expect(stop).toContain('<Button size="sm"');
		expect(stop).not.toMatch(/variant=|tone=/);
		expect(stop).toContain('<Spinner size={12} label="" />');
		expect(stop).toContain('{m.drawer_stop()}');
		expect(stop).toContain('onclick={oninterrupt}');
	});

	it('moves the title to its own line when the bar collapses, never in a tile', () => {
		expect(header).toContain('const titleOwnLine = $derived(collapsed && !compact)');
		expect(markup).toMatch(/\{#if titleOwnLine\}\s*<div class="trow">\{@render title\(\)\}<\/div>/);
		expect(markup.split('<DrawerTitle')).toHaveLength(2);
	});

	it('leaves text size to Settings, not the drawer header', () => {
		expect(header).not.toContain('FontScalePicker');
	});

	it('keeps the back chevron the one quiet affordance', () => {
		expect(named('m.drawer_back()')).not.toContain('chip');
	});

	it('makes the ⋯ trigger a single button so its tooltip stays on it', () => {
		const trigger = menu.slice(menu.indexOf('<Menu '), menu.indexOf('</Menu>'));
		expect(trigger).not.toContain('<IconButton');
		expect(trigger).toContain('data-journey="actions"');
		expect(trigger).toContain('title={m.drawer_more_actions()}');
	});
});
