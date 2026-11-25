// @vitest-environment happy-dom
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';

type Mod = typeof import('./spawnDock.svelte.ts');
let dock: Mod;
let settings: (typeof import('./settings.svelte.ts'))['settings'];

beforeAll(async () => {
	vi.stubGlobal(
		'matchMedia',
		(query: string) =>
			({
				matches: true,
				media: query,
				addEventListener: () => {},
				removeEventListener: () => {}
			}) as unknown as MediaQueryList
	);
	dock = await import('./spawnDock.svelte.ts');
	({ settings } = await import('./settings.svelte.ts'));
	settings.setSpawnDock({ enabled: true, side: 'left' });
});

afterEach(() => {
	dock.spawnDockHold.collapsed = false;
});

describe('spawn dock collapse', () => {
	it('docks the spawn form when nothing holds it', () => {
		expect(dock.spawnDockSide()).toBe('left');
		expect(dock.dockLayout().left).not.toBeNull();
	});

	it('folds the spawn form back to the button and frees its edge while collapsed', () => {
		dock.spawnDockHold.collapsed = true;
		expect(dock.spawnDockSide()).toBeNull();
		expect(dock.dockLayout().left).toBeNull();
		expect(settings.spawnDock.enabled).toBe(true);
	});
});
