import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import DrawerToolbar from './DrawerToolbar.svelte';
import toolbarSource from './DrawerToolbar.svelte?raw';
import lineSource from './ConversationLine.svelte?raw';
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
			mobilePanel: null,
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
	it('renders both chips as bare triggers carrying the shared chip class', async () => {
		const bar = await render();
		const chips = bar.querySelectorAll('.pop-trigger.toolbar-chip');
		expect(chips.length).toBe(2);
		for (const c of chips) expect(c.classList.contains('bare')).toBe(true);
		// Filters rides with the pill quick chips; Pins with the square toggles.
		expect(chips[0].classList.contains('toolbar-chip-pill')).toBe(true);
		expect(chips[1].classList.contains('toolbar-chip-pill')).toBe(false);
	});

	it('drops the ad-hoc override string and the local label span', () => {
		expect(toolbarSource).not.toContain('chipTrigger');
		expect(toolbarSource).not.toContain('chip-label');
		expect(toolbarSource).not.toContain('--pop-trigger-');
		expect(toolbarSource).not.toContain('--pop-box');
	});

	it('restates every chrome declaration a Toggle sets', () => {
		const chrome = toolbarSource.slice(
			toolbarSource.indexOf(':global(.pop-trigger.toolbar-chip)'),
			toolbarSource.indexOf('/* Filters sits among')
		);
		for (const decl of [
			'padding: 0.15rem var(--sp-2)',
			'border: 1px solid var(--border)',
			'border-radius: var(--r-sm)',
			'background: var(--bg-elevated-2)',
			'color: var(--text-muted)',
			'font-size: var(--fs-xs)',
			'font-weight: var(--fw-medium)',
			'line-height: 1.4'
		]) {
			expect(chrome).toContain(decl);
		}
	});
});

describe('pin glyph', () => {
	it('uses the kit pin icon, not a star, in the toolbar', async () => {
		const bar = await render();
		const chips = [...bar.querySelectorAll('.pop-trigger.toolbar-chip')];
		const pinChip = chips[chips.length - 1];
		expect(pinChip.textContent).not.toContain('★');
		expect(pinChip.querySelector('svg[data-tsu="Icon"]')).not.toBeNull();
	});

	it('renders outline when there are no pins and filled once there are', async () => {
		let bar = await render();
		let icon = [...bar.querySelectorAll('.pop-trigger.toolbar-chip svg')].pop() as SVGElement;
		expect(icon.getAttribute('fill')).toBe('none');

		if (comp) unmount(comp);
		document.body.innerHTML = '';
		bar = await render({ pins: [pin(3)] });
		icon = [...bar.querySelectorAll('.pop-trigger.toolbar-chip svg')].pop() as SVGElement;
		expect(icon.getAttribute('fill')).toBe('currentColor');
	});

	it('uses the same icon for the per-message action', () => {
		expect(lineSource).not.toContain("pinned ? '★' : '☆'");
		expect(lineSource).toContain('<Icon name="pin" size={16} filled={pinned} />');
	});
});
