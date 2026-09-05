import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
	CURRENT_VERSION,
	clampNavPosition,
	clampSessionListWidth,
	mergeDefaults,
	sessionListWidthSize,
	settings
} from './settings.svelte';
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
			sessionEmojiPrefix: true,
			autoResumeOnConnectionLoss: true,
			shortcutsEnabled: true,
			locale: 'en'
		} as Record<string, unknown>);
		const reloaded = mergeDefaults(JSON.parse(JSON.stringify(saved)) as Record<string, unknown>);
		expect(reloaded).toEqual(saved);
	});

	it('the nav position defaults to top, survives a reload, and clamps unknown values', () => {
		expect(mergeDefaults(null).display.nav).toBe('top');
		settings.setNav('bottom');
		expect(loadFromCache().display.nav).toBe('bottom');
		settings.setNav('top');
		expect(loadFromCache().display.nav).toBe('top');

		const merged = mergeDefaults({ display: { nav: 'sideways' } } as unknown as Record<string, unknown>);
		expect(merged.display.nav).toBe('top');
		expect(clampNavPosition(undefined)).toBe('top');
	});

	it('groupBy defaults to status and migrates the legacy none', () => {
		expect(mergeDefaults(null).sessionList.groupBy).toBe('status');
		const legacy = mergeDefaults({ sessionList: { groupBy: 'none' } } as unknown as Record<string, unknown>);
		expect(legacy.sessionList.groupBy).toBe('status');
		expect(mergeDefaults({ sessionList: { groupBy: 'machine' } } as Record<string, unknown>).sessionList.groupBy).toBe('machine');
		settings.setSessionList({ groupBy: 'label' });
		expect(loadFromCache().sessionList.groupBy).toBe('label');
	});

	it('list width and account-name toggle survive a persist then reload', () => {
		settings.setSessionList({ width: 'full', accountNames: true });

		const loaded = loadFromCache();
		expect(loaded.sessionList.width).toBe('full');
		expect(loaded.sessionList.accountNames).toBe(true);
	});

	it('an unknown stored width clamps to the default and account names default off', () => {
		const merged = mergeDefaults({
			sessionList: { width: 'gigantic' }
		} as unknown as Record<string, unknown>);
		expect(merged.sessionList.width).toBe('default');
		expect(merged.sessionList.accountNames).toBe(false);
		expect(clampSessionListWidth(undefined)).toBe('default');
	});

	it('the docked spawn panel toggle and side survive a persist then reload', () => {
		settings.setSpawnDock({ enabled: true, side: 'left' });

		const loaded = loadFromCache();
		expect(loaded.spawnDock.enabled).toBe(true);
		expect(loaded.spawnDock.side).toBe('left');
	});

	it('the docked spawn panel defaults off on the right and clamps an unknown side', () => {
		expect(mergeDefaults(null).spawnDock).toEqual({ enabled: false, side: 'right' });
		const merged = mergeDefaults({
			spawnDock: { enabled: 'yes', side: 'top' }
		} as unknown as Record<string, unknown>);
		expect(merged.spawnDock.enabled).toBe(false);
		expect(merged.spawnDock.side).toBe('right');
	});

	it('the toast position survives a persist then reload', () => {
		settings.setToastPosition('right');

		expect(loadFromCache().toastPosition).toBe('right');
	});

	it('the toast position defaults to center and clamps an unknown value', () => {
		expect(mergeDefaults(null).toastPosition).toBe('center');
		const merged = mergeDefaults({ toastPosition: 'bottom' } as unknown as Record<
			string,
			unknown
		>);
		expect(merged.toastPosition).toBe('center');
	});

	it('the docked stats panel survives a persist then reload', () => {
		settings.setStatsDock({ enabled: true, side: 'left' });

		const loaded = loadFromCache();
		expect(loaded.statsDock.enabled).toBe(true);
		expect(loaded.statsDock.side).toBe('left');
	});

	it('the docked stats panel defaults off on the right and clamps an unknown side', () => {
		expect(mergeDefaults(null).statsDock).toEqual({ enabled: false, side: 'right' });
		const merged = mergeDefaults({
			statsDock: { enabled: 1, side: 'bottom' }
		} as unknown as Record<string, unknown>);
		expect(merged.statsDock.enabled).toBe(false);
		expect(merged.statsDock.side).toBe('right');
	});

	it('each width maps to a CSS length, the default keeping --content-wide', () => {
		expect(sessionListWidthSize('default')).toBeUndefined();
		expect(sessionListWidthSize('wide')).toBe('80rem');
		expect(sessionListWidthSize('ultra')).toBe('92rem');
		expect(sessionListWidthSize('full')).toBe('100%');
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
