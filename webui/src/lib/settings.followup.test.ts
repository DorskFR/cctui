// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { mergeDefaults, type SettingsState } from './settings.svelte';

const load = (display: Record<string, unknown> | undefined) =>
	mergeDefaults((display ? { display } : {}) as unknown as Partial<SettingsState>).display;

describe('followupWhenCold settings migration', () => {
	it('defaults to auto', () => {
		expect(load(undefined).coldFollowup).toBe('auto');
	});

	it('migrates the stored offer to auto and default to modal, unknown to auto, and keeps off', () => {
		expect(load({ followupWhenCold: 'offer' }).coldFollowup).toBe('auto');
		expect(load({ followupWhenCold: 'default' }).coldFollowup).toBe('modal');
		expect(load({ followupWhenCold: 'whatever' }).coldFollowup).toBe('auto');
		expect(load({ followupWhenCold: 'off' }).coldFollowup).toBe('off');
	});

	it('drops the legacy key so a later save keeps the chosen value', () => {
		const migrated = load({ followupWhenCold: 'default' });
		expect(migrated.followupWhenCold).toBeUndefined();
		expect(load({ ...migrated, coldFollowup: 'offer' }).coldFollowup).toBe('offer');
	});
});
