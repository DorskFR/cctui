// @vitest-environment happy-dom
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';

vi.hoisted(() => {
	window.matchMedia = ((query: string) => ({
		matches: true,
		media: query,
		addEventListener: () => {},
		removeEventListener: () => {}
	})) as unknown as typeof window.matchMedia;
});

import { dockLayout, spawnDockHold, spawnDockSide } from './spawnDock.svelte.ts';
import { settings } from './settings.svelte.ts';

beforeAll(() => {
	settings.setSpawnDock({ enabled: true, side: 'left' });
});

afterEach(() => {
	spawnDockHold.collapsed = false;
});

describe('spawn dock collapse', () => {
	it('docks the spawn form when nothing holds it', () => {
		expect(spawnDockSide()).toBe('left');
		expect(dockLayout().left).not.toBeNull();
	});

	it('folds the spawn form back to the button and frees its edge while collapsed', () => {
		spawnDockHold.collapsed = true;
		expect(spawnDockSide()).toBeNull();
		expect(dockLayout().left).toBeNull();
		expect(settings.spawnDock.enabled).toBe(true);
	});
});
