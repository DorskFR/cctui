// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import DrawerToolbar from './DrawerToolbar.svelte';
import toolbarSource from './DrawerToolbar.svelte?raw';
import lineSource from './LineActions.svelte?raw';
import headerSource from './DrawerHeader.svelte?raw';
import { allFilter } from './filters';
import type { ViewOpts } from './types';
import type { MessagePin } from '@bindings/MessagePin';

const flush = () => new Promise((r) => setTimeout(r, 0));

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const view = (): ViewOpts =>
	({
		msgFilter: allFilter(true),
		prettyJson: true,
		prettyDiff: true,
		prettyTables: true,
		paneWidth: null
	}) as ViewOpts;

const pin = (seq: number): MessagePin => ({
	session_id: 's1',
	seq,
	message_id: null,
	note: null,
	created_at: '2026-01-01T00:00:00Z'
});

async function render(extra: Record<string, unknown> = {}) {
	comp = mount(DrawerToolbar, {
		target: document.body,
		props: {
			view: view(),
			autoApprove: false,
			ontoggleAuto: vi.fn(),
			onjumpseq: vi.fn(),
			onunpin: vi.fn(),
			...extra
		}
	});
	await flush();
	return document.querySelector('.toolbar') as HTMLElement;
}

describe('wrap-up bookmark shortcut', () => {
	it('is gone from the toolbar', () => {
		expect(toolbarSource).not.toContain('onbookmarkwrapup');
		expect(toolbarSource).not.toContain('bookmarks_toolbar');
	});
});

describe('popover triggers match their sibling toggles', () => {
	it('renders both triggers as the kit Toggle chip itself', async () => {
		const bar = await render();
		const triggers = [...bar.querySelectorAll('[data-tsu="Popover"]')];
		expect(triggers.length).toBe(2);
		for (const t of triggers) {
			expect(t.classList.contains('toggle')).toBe(true);
			expect(t.classList.contains('trigger-toggle')).toBe(true);
			expect(t.classList.contains('bare')).toBe(false);
		}
		// Filters rides with the pill quick chips; Pins with the square toggles.
		expect(triggers[0].classList.contains('pill')).toBe(true);
		expect(triggers[1].classList.contains('pill')).toBe(false);
	});

	it('keeps no local chip chrome to drift from the kit Toggle', () => {
		expect(toolbarSource).not.toContain(':global(');
		expect(toolbarSource).not.toContain('toolbar-chip');
		expect(toolbarSource).not.toContain('\t.chip {');
	});

	it('drops the ad-hoc override string and the local label span', () => {
		expect(toolbarSource).not.toContain('chipTrigger');
		expect(toolbarSource).not.toContain('chip-label');
		expect(toolbarSource).not.toContain('--pop-trigger-');
		expect(toolbarSource).not.toContain('--pop-box');
	});
});

describe('pin glyph', () => {
	it('uses the kit pin icon, not a star, in the toolbar', async () => {
		const bar = await render();
		const chips = [...bar.querySelectorAll('[data-tsu="Popover"]')];
		const pinChip = chips[chips.length - 1];
		expect(pinChip.textContent).not.toContain('★');
		expect(pinChip.querySelector('svg[data-tsu="Icon"]')).not.toBeNull();
	});

	it('renders outline when there are no pins and filled once there are', async () => {
		let bar = await render();
		let icon = [...bar.querySelectorAll('[data-tsu="Popover"] svg')].pop() as SVGElement;
		expect(icon.getAttribute('fill')).toBe('none');

		if (comp) unmount(comp);
		document.body.innerHTML = '';
		bar = await render({ pins: [pin(3)] });
		icon = [...bar.querySelectorAll('[data-tsu="Popover"] svg')].pop() as SVGElement;
		expect(icon.getAttribute('fill')).toBe('currentColor');
	});

	it('uses the same icon for the per-message action', () => {
		expect(lineSource).not.toContain("pinned ? '★' : '☆'");
		expect(lineSource).toContain("icon: pinned ? 'pin-off' : 'pin'");
		expect(lineSource).not.toContain('★');
	});
});

describe('panels menu', () => {
	const trigger = (bar: HTMLElement) => bar.querySelector('[data-journey="panels"]') as HTMLElement | null;

	it('is hidden when there is no panel to open', async () => {
		const bar = await render({});
		expect(trigger(bar)).toBeNull();
	});

	it('is one labelled trigger instead of a row of unlabelled toggles', async () => {
		const bar = await render({
			ontoggleTerminal: vi.fn(),
			plugins: [{ id: 'demo', label: 'yubisashi', icon: 'eye', open: false, onselect: vi.fn() }]
		});
		const t = trigger(bar);
		expect(t).not.toBeNull();
		expect(t?.textContent).toContain('Panels');
		expect(bar.querySelector('.behbar > [data-journey="terminal"]')).toBeNull();
		expect(bar.querySelector('.behbar > [data-journey="plugin"]')).toBeNull();
	});

	it('lists the terminal and every plugin pane as checkable entries that keep the menu open', () => {
		const items = toolbarSource.slice(
			toolbarSource.indexOf('const panelItems'),
			toolbarSource.indexOf('const panelsOpen')
		);
		expect(items).toContain('pressed: terminalOpen');
		expect(items).toContain('pressed: p.open');
		expect(items.match(/keepOpen: true/g)?.length).toBe(2);
		expect(items).toContain("'data-journey': 'terminal'");
		expect(items).toContain("'data-journey': 'plugin'");
		expect(items).toContain("'data-plugin': p.id");
		expect(items).toContain('onselect: ontoggleTerminal');
		expect(items).toContain('onselect: p.onselect');
	});
});

describe('auto-approve', () => {
	it('is a switch that says its state in words', async () => {
		const bar = await render({ autoApprove: false });
		const sw = bar.querySelector('[data-journey="auto-approve"] [role="switch"]') as HTMLElement;
		expect(sw).not.toBeNull();
		expect(sw.getAttribute('aria-checked')).toBe('false');
		expect(bar.querySelector('[data-journey="auto-approve"]')?.textContent).toContain('Auto-approve: off');
		expect(toolbarSource).not.toContain('⚡');
	});

	it('reads on when the session auto-approves, and asks the parent to flip it', async () => {
		const ontoggleAuto = vi.fn();
		const bar = await render({ autoApprove: true, ontoggleAuto });
		const sw = bar.querySelector('[data-journey="auto-approve"] [role="switch"]') as HTMLElement;
		expect(sw.getAttribute('aria-checked')).toBe('true');
		expect(bar.textContent).toContain('Auto-approve: on');
		sw.click();
		expect(ontoggleAuto).toHaveBeenCalledOnce();
	});
});

describe('filter pills', () => {
	it('share one neutral selected style with a check, no role tint', async () => {
		expect(toolbarSource).not.toContain('QUICK_TINT');
		expect(toolbarSource).not.toContain('--role-');
		expect(toolbarSource).toContain("const PILL = `${CTL};--toggle-accent: var(--text)`");
		const bar = await render({ view: { ...view(), msgFilter: { ...allFilter(true), user: false } } });
		const pills = [...bar.querySelectorAll('[data-journey="quick"]')] as HTMLElement[];
		expect(pills).toHaveLength(3);
		for (const p of pills) {
			const on = p.getAttribute('aria-pressed') === 'true';
			expect(!!p.querySelector('svg[data-tsu="Icon"]'), p.textContent ?? '').toBe(on);
		}
		expect(pills.find((p) => p.dataset.journeyKey === 'user')?.getAttribute('aria-pressed')).toBe('false');
	});
});

describe('single-row toolbar', () => {
	it('has no format toggles, no diagnose and no mobile tabs', async () => {
		const bar = await render({});
		expect(bar.textContent).not.toMatch(/JSON|Diff|Tables|Diagnose/);
		expect(bar.querySelector('[data-journey="mobile-panel"]')).toBeNull();
		expect(toolbarSource).not.toContain('mobilePanel');
	});

	it('shows how many categories are hidden next to the filter icon', async () => {
		const bar = await render({
			view: { ...view(), msgFilter: { ...allFilter(true), thinking: false, marker: false } }
		});
		const trigger = bar.querySelector('[data-journey="filter-menu"]') as HTMLElement;
		expect(trigger.querySelector('svg')).toBeTruthy();
		expect(trigger.querySelector('.narrow')?.textContent).toBe('2');
	});
});

describe('drawer toolbar sizing', () => {
	const css = toolbarSource.slice(toolbarSource.indexOf('<style>'));

	it('switches form on the bar width, not the viewport', () => {
		expect(css).toContain('container: drawer-toolbar / inline-size');
		expect(css).toContain('@container drawer-toolbar (max-width: 1000px)');
		expect(css).not.toContain('@media');
	});

	it('drops the labels in the compact form and right-aligns the behaviour group at every width', () => {
		const q = css.slice(css.indexOf('@container drawer-toolbar'));
		const body = q.slice(0, q.indexOf('\n\t}'));
		expect(body).toContain('.wide {');
		expect(body).toContain('.narrow {');
		expect(body).not.toContain('margin-left: auto');
		expect(css.slice(0, css.indexOf('@container'))).toContain('margin-left: auto;');
	});

	it('never lets auto-approve or pins be the group that clips', () => {
		expect(css).toMatch(/\.behbar \{\n\t\tflex: none;/);
		const tag = css.slice(css.indexOf('.tagbar {'));
		const body = tag.slice(0, tag.indexOf('}'));
		expect(body).toContain('min-width: 0');
		expect(body).toContain('overflow: hidden');
	});

	it('pins every control in the bar to one height', () => {
		expect(css).toContain('--bar-ctl-h: 24px');
		expect(toolbarSource).toContain(
			"const CTL = 'height:var(--bar-ctl-h);box-sizing:border-box;padding-block:0;line-height:1'"
		);
		expect(toolbarSource).not.toContain('const TRIG');
	});

	it('gives every toggle and popover trigger the same pinned box', () => {
		const toggles = toolbarSource.match(/<Toggle\b[\s\S]*?>/g) ?? [];
		expect(toggles.length).toBeGreaterThanOrEqual(1);
		for (const t of toggles) expect(t, t.slice(0, 60)).toMatch(/\$\{PILL\}/);
		const triggers = toolbarSource.match(/<(Popover|Menu)\b[\s\S]*?>/g) ?? [];
		expect(triggers.length).toBe(3);
		for (const p of triggers) expect(p).toContain('variant="toggle"');
		for (const p of triggers) expect(p).toContain('style={CTL}');
	});

	it('matches the two chip icons in size', () => {
		expect(toolbarSource).toContain('<Icon name="filter" size={12} />');
		expect(toolbarSource).toContain('<Icon name="pin" size={12}');
	});

	it('keeps auto-approve and pins named and right-aligned', () => {
		expect(toolbarSource).toContain('title={m.conversation_auto_approve_title()}');
		expect(toolbarSource).toContain('label={m.conversation_pins_aria()}');
		const behbarRules = css
			.split('.behbar {')
			.slice(1)
			.map((r) => r.slice(0, r.indexOf('}')));
		expect(behbarRules.length).toBeGreaterThan(0);
		expect(behbarRules.some((r) => r.includes('margin-left: auto;'))).toBe(true);
	});

	it('wraps to two rows on a phone instead of clipping', () => {
		const q = css.slice(css.indexOf('@container drawer-toolbar (max-width: 560px)'));
		expect(q).toContain('.tbrow {\n\t\t\tflex-wrap: wrap;');
	});

	it('adds no :global override', () => {
		expect(toolbarSource).not.toContain(':global(');
	});
});

describe('plugin pane toggles', () => {
	// The header does import `$lib/plugins/*` for the per-session issue slot, so
	// this guards the pane list and the toggle specifically, not the substring.
	it('render nowhere else: the header lost its copy', () => {
		expect(headerSource).not.toContain('data-journey="plugin"');
		expect(headerSource).not.toContain('PluginButton');
		expect(headerSource).not.toMatch(/^\s*plugins[,:=]/m);
		expect(headerSource).not.toMatch(/\{#each plugins\b/);
	});
});
