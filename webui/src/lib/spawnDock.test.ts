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

import { SPAWN_DOCK_RAIL_WIDTH } from './dock.ts';
import {
	dockLayout,
	expandSpawnDock,
	minifySpawnDock,
	spawnDockHold,
	spawnDockMinified,
	spawnDockSide
} from './spawnDock.svelte.ts';
import { settings } from './settings.svelte.ts';

beforeAll(() => {
	settings.setSpawnDock({ enabled: true, side: 'left' });
});

afterEach(() => {
	spawnDockHold.chat = false;
	spawnDockHold.expanded = false;
	settings.setSpawnDock({ foldOnChat: true, minified: false });
});

describe('spawn dock minify', () => {
	it('docks the full form when nothing minifies it', () => {
		expect(spawnDockSide()).toBe('left');
		expect(dockLayout().spawnMinified).toBe(false);
		expect(dockLayout().left).not.toBe(SPAWN_DOCK_RAIL_WIDTH);
	});

	it('minifies to the rail on click and keeps it across reloads', () => {
		minifySpawnDock();
		expect(settings.spawnDock.minified).toBe(true);
		expect(spawnDockSide()).toBe('left');
		expect(dockLayout().left).toBe(SPAWN_DOCK_RAIL_WIDTH);
		expandSpawnDock();
		expect(settings.spawnDock.minified).toBe(false);
		expect(dockLayout().spawnMinified).toBe(false);
	});

	it('minifies to the rail while a conversation is open, keeping its edge', () => {
		spawnDockHold.chat = true;
		expect(spawnDockMinified()).toBe(true);
		expect(spawnDockSide()).toBe('left');
		expect(dockLayout().left).toBe(SPAWN_DOCK_RAIL_WIDTH);
		expect(settings.spawnDock.minified).toBe(false);
		spawnDockHold.chat = false;
		expect(spawnDockMinified()).toBe(false);
	});

	it('stays expanded during a conversation when folding is turned off', () => {
		settings.setSpawnDock({ foldOnChat: false });
		spawnDockHold.chat = true;
		expect(spawnDockMinified()).toBe(false);
	});

	it('expanding during a conversation does not touch the saved state, and minifying refolds it', () => {
		spawnDockHold.chat = true;
		expandSpawnDock();
		expect(spawnDockMinified()).toBe(false);
		minifySpawnDock();
		expect(spawnDockMinified()).toBe(true);
		expect(settings.spawnDock.minified).toBe(false);
		spawnDockHold.chat = false;
		expect(spawnDockMinified()).toBe(false);
	});

	it('stays minified after the conversation when the user had minified it before', () => {
		minifySpawnDock();
		spawnDockHold.chat = true;
		expect(spawnDockMinified()).toBe(true);
		spawnDockHold.chat = false;
		expect(spawnDockMinified()).toBe(true);
	});
});
