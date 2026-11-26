// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import { IconButton } from '@dorsk/tsumikit';
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

	it('always offers Stop beside Archive as one red icon that spins until acknowledged', () => {
		expect(header).not.toContain('canStop');
		expect(header).not.toMatch(/\bworking\b/);
		const block = markup.slice(markup.indexOf('{#if !archived && !compact}'), markup.indexOf('{/if}', markup.indexOf('{#if !archived && !compact}')));
		expect(block).toContain('icon="archive"');
		const stop = block.slice(block.indexOf('icon="stop"') - 80);
		expect(stop).toContain('tone="danger"');
		expect(stop).toContain('spin={stopping}');
		expect(stop).not.toContain('loading=');
		expect(stop).toContain('onclick={stop}');
		expect(header).toMatch(/stopping = true;\s*try \{\s*await oninterrupt\(\);\s*\} finally \{\s*stopping = false;/);
	});

	it('turns the stop square itself while stopping, with no spinner beside it', () => {
		const target = document.createElement('div');
		document.body.append(target);
		const c = mount(IconButton, {
			target,
			props: { icon: 'stop', label: 'Stop', tone: 'danger', variant: 'ghost', box: 'md', spin: true }
		});
		flushSync();
		const btn = target.querySelector('button') as HTMLButtonElement;
		expect(btn.querySelectorAll('svg')).toHaveLength(1);
		expect(btn.querySelector('.btn-spinner')).toBeNull();
		expect(btn.querySelector('svg')?.classList.contains('spin')).toBe(true);
		unmount(c);
		target.remove();
	});

	it('orders Stop before Archive', () => {
		expect(markup.indexOf('icon="stop"')).toBeLessThan(markup.indexOf('icon="archive"'));
	});

	it('keeps the title in the one action row at every width', () => {
		expect(header).not.toContain('titleOwnLine');
		expect(markup).not.toContain('class="trow"');
		expect(markup.split('<DrawerTitle')).toHaveLength(2);
		expect(markup.indexOf('<DrawerTitle')).toBeLessThan(markup.indexOf('</Toolbar>'));
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
