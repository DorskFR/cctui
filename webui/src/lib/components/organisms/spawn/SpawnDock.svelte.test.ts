// @vitest-environment happy-dom
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';

vi.hoisted(() => {
	window.matchMedia = ((query: string) => ({
		matches: true,
		media: query,
		addEventListener: () => {},
		removeEventListener: () => {}
	})) as unknown as typeof window.matchMedia;
});

import { m } from '#lib/paraglide/messages.js';
import { settings } from '#lib/settings.svelte.ts';
import { spawnDockHold } from '#lib/spawnDock.svelte.ts';
import Host from './SpawnDock.host.test.svelte';

let component: ReturnType<typeof mount> | undefined;

beforeAll(() => {
	settings.setSpawnDock({ enabled: true, side: 'left' });
});

afterEach(() => {
	if (component) unmount(component);
	component = undefined;
	spawnDockHold.chat = false;
	spawnDockHold.expanded = false;
	settings.setSpawnDock({ foldOnChat: true, minified: false });
	document.body.innerHTML = '';
});

function render(side: 'left' | 'right' = 'left') {
	component = mount(Host, { target: document.body, props: { side } });
	flushSync();
}

const button = (label: string) =>
	document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`);
const draft = () => document.querySelector<HTMLTextAreaElement>('textarea[name="draft"]');
const dock = () => document.querySelector<HTMLElement>('aside.dock');

describe('SpawnDock minify', () => {
	it('minifies to the rail from the header and expands back with the draft intact', async () => {
		render();
		const input = draft();
		if (!input) throw new Error('no form rendered');
		input.value = 'half-written prompt';

		const minify = button(m.spawn_dock_minify());
		expect(minify?.getAttribute('aria-expanded')).toBe('true');
		expect(minify?.textContent).toContain('«');
		minify?.click();
		flushSync();
		await tick();

		expect(dock()?.classList.contains('minified')).toBe(true);
		expect(settings.spawnDock.minified).toBe(true);
		expect(input.closest('.dock-body')?.hasAttribute('hidden')).toBe(true);
		expect(document.querySelector('.grip')).toBeNull();
		const expand = button(m.spawn_dock_expand());
		expect(expand?.getAttribute('aria-expanded')).toBe('false');
		expect(expand?.getAttribute('aria-controls')).toBe(input.closest('.dock-body')?.id);
		expect(document.activeElement).toBe(expand);

		expand?.click();
		flushSync();
		await tick();

		expect(dock()?.classList.contains('minified')).toBe(false);
		expect(settings.spawnDock.minified).toBe(false);
		expect(draft()).toBe(input);
		expect(input.value).toBe('half-written prompt');
		expect(document.activeElement).toBe(button(m.spawn_dock_minify()));
	});

	it('points the toggles toward the edge on a right dock', () => {
		render('right');
		expect(button(m.spawn_dock_minify())?.textContent).toContain('»');
		button(m.spawn_dock_minify())?.click();
		flushSync();
		expect(button(m.spawn_dock_expand())?.textContent).toContain('«');
	});

	it('shows a single expand button on the rail', () => {
		settings.setSpawnDock({ minified: true });
		render();
		const buttons = [...document.querySelectorAll<HTMLButtonElement>('.rail button')];
		expect(buttons.map((b) => b.getAttribute('aria-label'))).toEqual([m.spawn_dock_expand()]);
		expect(buttons[0].title).toBe(m.spawn_dock_expand());
	});

	it('minifies while a conversation is open and restores the form when it closes', () => {
		render();
		spawnDockHold.chat = true;
		flushSync();
		expect(dock()?.classList.contains('minified')).toBe(true);
		expect(settings.spawnDock.minified).toBe(false);
		spawnDockHold.chat = false;
		flushSync();
		expect(dock()?.classList.contains('minified')).toBe(false);
	});
});
