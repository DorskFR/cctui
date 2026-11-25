// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import header from './DrawerHeader.svelte?raw';
import menu from './DrawerMenu.svelte?raw';

const markup = header.slice(header.indexOf('</script>'));
const controls = markup
	.split(/(?=<IconButton|<FontScalePicker|<DrawerMenu)/)
	.filter((c) => /^<(IconButton|FontScalePicker|DrawerMenu)/.test(c))
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

	it('gives every trailing control the same chip chrome', () => {
		for (const label of ['m.drawer_archive()', 'm.drawer_interrupt_label()', 'm.drawer_rename()']) {
			expect(named(label)).toContain('chip');
		}
		expect(markup).not.toContain('chip={!collapsed}');
		for (const label of ['<FontScalePicker', '<DrawerMenu']) {
			expect(named(label)).toContain('style={CHIP_CHROME}');
		}
		expect(menu).toContain('placement="bottom-end" {box} {style}');
		expect(header).toContain("const CHIP_CHROME = 'border-color: var(--border-strong); background: var(--surface)'");
		expect(header).not.toContain('--pop-trigger-');
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
