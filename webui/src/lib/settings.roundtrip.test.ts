import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { CURRENT_VERSION, mergeDefaults, settings } from './settings.svelte';
import { auth } from './auth.svelte';

const KEY = 'cctui_settings';

function loadFromCache() {
	const raw = localStorage.getItem(KEY);
	return mergeDefaults(raw ? (JSON.parse(raw) as Record<string, unknown>) : null);
}

beforeEach(() => {
	auth.isAuthed = false; // keep persist() cache-only (no server PUT)
	localStorage.clear();
});

afterEach(() => {
	localStorage.clear();
});

describe('Settings save → load round-trip through the blob', () => {
	it('theme / fontScale / notify / locale survive a persist then reload', () => {
		settings.setDisplay({
			theme: 'sepia',
			fontScale: 1.25,
			notifyEnabled: true,
			notifySound: false
		});
		settings.setLocale('fr');

		const loaded = loadFromCache();
		expect(loaded.display.theme).toBe('sepia');
		expect(loaded.display.fontScale).toBe(1.25);
		expect(loaded.display.notifyEnabled).toBe(true);
		expect(loaded.display.notifySound).toBe(false);
		expect(loaded.locale).toBe('fr');
	});

	it('a full serialize → mergeDefaults preserves the whole catalogue', () => {
		const saved = mergeDefaults({
			sessionList: { sort: 'name', view: 'card', density: 'compact' },
			display: { theme: 'light', fontScale: 0.9, archiveShortcut: false },
			harnessMode: 'sdk',
			whipStopPhrases: { mode: 'replace', phrases: ['stop now'], guidance: 'go' },
			secretScrubEnabled: true,
			secretScrubPatterns: [{ name: 'tok', regex: 'sk-\\w+', enabled: true }],
			shortcutsEnabled: true,
			locale: 'en'
		} as Record<string, unknown>);
		const reloaded = mergeDefaults(JSON.parse(JSON.stringify(saved)) as Record<string, unknown>);
		expect(reloaded).toEqual(saved);
	});

	it('a persisted blob keyed as version restores through the setter path', () => {
		settings.setHarnessMode('oneshot');
		const loaded = loadFromCache();
		expect(loaded.harnessMode).toBe('oneshot');
		expect(CURRENT_VERSION).toBe(1);
	});

	it('missing fields in an older blob fall back to defaults, not undefined', () => {
		const loaded = mergeDefaults({ display: { theme: 'light' } } as Record<string, unknown>);
		expect(loaded.display.theme).toBe('light');
		expect(loaded.display.fontScale).toBe(1);
		expect(loaded.display.notifySound).toBe(true);
		expect(loaded.locale).toBeNull();
		expect(loaded.harnessMode).toBe('bg');
	});

	it('clamps an unknown harness mode / locale on load', () => {
		const loaded = mergeDefaults({
			harnessMode: 'bogus',
			locale: 'zz'
		} as unknown as Record<string, unknown>);
		expect(loaded.harnessMode).toBe('bg');
		expect(loaded.locale).toBeNull();
	});
});
