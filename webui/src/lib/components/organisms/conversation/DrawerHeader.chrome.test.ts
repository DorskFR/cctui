// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import header from './DrawerHeader.svelte?raw';

const markup = header.slice(header.indexOf('</script>'));
const bar = markup.slice(markup.indexOf('<Toolbar'), markup.indexOf('</Toolbar>'));
const controls = bar
	.split(/(?=<IconButton|<Button|<Menu )/)
	.filter((c) => /^<(IconButton|Button|Menu )/.test(c))
	.map((c) => c.slice(0, c.indexOf('>') + 1));

const named = (label: string) => {
	const c = controls.find((x) => x.includes(label));
	expect(c, label).toBeTruthy();
	return c as string;
};

describe('drawer header control chrome', () => {
	it('sizes every control from one box per density', () => {
		expect(header).toContain("const box: 'sm' | 'md' = $derived(collapsed ? 'sm' : 'md')");
		for (const c of controls) expect(c, c).toMatch(/\{box\}|size=\{box\}/);
		expect(bar).not.toMatch(/box="(xs|sm|md|lg)"/);
	});

	it('gives the icon actions the chip chrome and the ⋯ trigger the matching popover chrome', () => {
		for (const label of ['m.conversation_search_label()', 'm.common_save()']) {
			expect(named(label)).toContain('chip');
		}
		expect(named('m.drawer_more_actions()')).toContain('style={CHIP_CHROME}');
		expect(header).toContain('--pop-trigger-border: var(--border-strong)');
	});

	it('keeps the back chevron the one quiet affordance', () => {
		expect(named('m.drawer_back()')).not.toContain('chip');
	});

	it('makes the ⋯ trigger a single button so its tooltip stays on it', () => {
		const trigger = markup.slice(markup.indexOf('<Menu '), markup.indexOf('</Menu>'));
		expect(trigger).not.toContain('<IconButton');
		expect(trigger).toContain('data-journey="actions"');
		expect(trigger).toContain('title={m.drawer_more_actions()}');
	});

	it('makes every header and meta control a full touch target on coarse pointers', () => {
		const css = header.slice(header.indexOf('<style>'));
		const coarse = css.slice(css.indexOf('@media (pointer: coarse)'));
		const body = coarse.slice(0, coarse.indexOf('\n\t}'));
		for (const v of ['--box-sm', '--box-md', '--box-lg']) expect(body).toContain(`${v}: var(--hit-min);`);
		expect(body).toContain('--touch-slab: min(-8px, calc((100% - var(--hit-min)) / 2));');
		expect(body).toMatch(/\.dhead \{/);
	});

	it('has no filled or red control in the bar', () => {
		expect(bar).not.toContain('variant="primary"');
		expect(bar).not.toContain('tone="danger"');
		expect(bar).not.toContain('tone="warn"');
	});
});
