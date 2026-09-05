import { describe, expect, it } from 'vitest';
import type { EnvVar } from '@bindings/EnvVar';
import type { SettingKey } from '@bindings/SettingKey';
import type { SettingsCatalogResponse } from '@bindings/SettingsCatalogResponse';
import {
	applyPreset,
	getKnob,
	knobEnvNames,
	knobGroups,
	knobKeyNames,
	overriddenCount,
	setKnob
} from './knobs.logic';

const key = (over: Partial<SettingKey>): SettingKey => ({
	name: 'k',
	tag: 'safe',
	source: 'schema',
	type: 'boolean',
	enum: null,
	default: null,
	notes: null,
	group: null,
	label: null,
	...over
});

const env = (over: Partial<EnvVar>): EnvVar => ({
	name: 'E',
	group: 'model',
	tag: 'safe',
	kind: 'string',
	values: null,
	settings_equiv: null,
	env_alias_of: null,
	label: null,
	notes: null,
	...over
});

const catalog = (over: Partial<SettingsCatalogResponse>): SettingsCatalogResponse => ({
	keys: [],
	env: [],
	preset: { id: 'quiet', name: 'Quiet', description: '', settings: {}, env: {} },
	...over
});

describe('knobGroups', () => {
	it('merges an env twin into its settings row and pages the group', () => {
		const groups = knobGroups(
			catalog({
				keys: [key({ name: 'cleanupPeriodDays', group: 'Privacy & memory', label: 'Cleanup' })],
				env: [env({ name: 'CLEANUP', settings_equiv: 'cleanupPeriodDays', group: 'telemetry' })]
			})
		);
		expect(groups).toHaveLength(1);
		expect(groups[0].page).toBe('privacy');
		expect(groups[0].knobs[0].sub).toBe('cleanupPeriodDays · CLEANUP');
	});

	it('drops exact env aliases and keeps the rest under their own group', () => {
		const groups = knobGroups(
			catalog({
				env: [
					env({ name: 'DISABLE_TELEMETRY', group: 'telemetry', kind: 'flag' }),
					env({ name: 'DO_NOT_TRACK', group: 'telemetry', env_alias_of: 'DISABLE_TELEMETRY' })
				]
			})
		);
		expect(groups.map((g) => g.knobs.map((k) => k.id))).toEqual([['e:DISABLE_TELEMETRY']]);
	});

	it('is empty without a catalog', () => {
		expect(knobGroups(undefined)).toEqual([]);
	});
});

describe('get/setKnob', () => {
	const tri = knobGroups(
		catalog({ keys: [key({ name: 'autoUpdates', group: 'Tools', label: 'Auto updates' })] })
	)[0].knobs[0];

	it('round-trips a tri-state through the settings blob', () => {
		expect(getKnob({}, tri)).toBe('');
		const on = setKnob({}, tri, 'true');
		expect(on).toEqual({ autoUpdates: true });
		expect(getKnob(on, tri)).toBe('true');
		expect(setKnob(on, tri, '')).toEqual({});
	});

	it('writes an env-only knob under settings.env and clears the blob', () => {
		const knob = knobGroups(catalog({ env: [env({ name: 'MAX_TOKENS', group: 'tokens' })] }))[0]
			.knobs[0];
		const set = setKnob({}, knob, '42');
		expect(set).toEqual({ env: { MAX_TOKENS: '42' } });
		expect(getKnob(set, knob)).toBe('42');
		expect(setKnob(set, knob, '')).toEqual({});
	});

	it('falls back to the env twin for a value the settings enum rejects', () => {
		const knob = knobGroups(
			catalog({
				keys: [key({ name: 'effortLevel', type: 'string', enum: 'low,high' })],
				env: [
					env({
						name: 'CLAUDE_CODE_EFFORT_LEVEL',
						group: 'thinking',
						kind: 'enum',
						values: ['low', 'high', 'ultra'],
						settings_equiv: 'effortLevel'
					})
				]
			})
		)[0].knobs[0];
		expect(setKnob({}, knob, 'high')).toEqual({ effortLevel: 'high' });
		expect(setKnob({}, knob, 'ultra')).toEqual({ env: { CLAUDE_CODE_EFFORT_LEVEL: 'ultra' } });
	});
});

describe('overriddenCount and applyPreset', () => {
	const groups = knobGroups(
		catalog({
			keys: [
				key({ name: 'autoUpdates', group: 'Tools', label: 'Auto updates' }),
				key({ name: 'verbose', group: 'Tools', label: 'Verbose' })
			],
			env: [env({ name: 'MAX_TOKENS', group: 'tokens' })]
		})
	);
	const toolKnobs = groups[0].knobs;

	it('counts only the knobs holding a non-default value', () => {
		expect(overriddenCount({ autoUpdates: false }, toolKnobs)).toBe(1);
		expect(overriddenCount({}, toolKnobs)).toBe(0);
	});

	it('applies only the preset entries this page owns', () => {
		const preset = {
			id: 'quiet',
			name: 'Quiet',
			description: '',
			settings: { autoUpdates: false, elsewhere: true },
			env: { MAX_TOKENS: '1' }
		};
		expect(applyPreset({}, preset, toolKnobs)).toEqual({ autoUpdates: false });
		expect(applyPreset({}, preset, groups[1].knobs)).toEqual({ env: { MAX_TOKENS: '1' } });
	});

	it('lists the settings and env names a page drives', () => {
		expect(knobKeyNames(toolKnobs)).toEqual(['autoUpdates', 'verbose']);
		expect(knobEnvNames(groups[1].knobs)).toEqual(['MAX_TOKENS']);
	});
});
