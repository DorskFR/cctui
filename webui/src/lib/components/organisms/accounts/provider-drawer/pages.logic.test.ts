import { describe, expect, it } from 'vitest';
import { diffCount, groupPage, looseSettings, pagesFor, settingsSlice, softFlat } from './pages.logic';

describe('pagesFor', () => {
	it('gives an anthropic credential the settings pages', () => {
		expect(pagesFor('anthropic')).toEqual([
			'aliases',
			'limits',
			'ui',
			'privacy',
			'tools',
			'gateway',
			'advanced'
		]);
	});

	it('drops the settings pages for codex and adds models for fireworks', () => {
		expect(pagesFor('openai')).toEqual(['aliases', 'limits', 'gateway', 'advanced']);
		expect(pagesFor('fireworks')).toEqual(['aliases', 'limits', 'models', 'gateway', 'advanced']);
	});

	it('keeps the settings pages and the model list for a compatible endpoint', () => {
		expect(pagesFor('anthropic-compatible')).toEqual([
			'aliases',
			'limits',
			'ui',
			'privacy',
			'tools',
			'models',
			'gateway',
			'advanced'
		]);
	});
});

describe('groupPage', () => {
	it('routes catalog groups, unknown ones to advanced', () => {
		expect(groupPage('UI & transcript')).toBe('ui');
		expect(groupPage('telemetry')).toBe('privacy');
		expect(groupPage('Editing & safety')).toBe('tools');
		expect(groupPage('timeouts')).toBe('gateway');
		expect(groupPage('something new')).toBe('advanced');
	});
});

describe('diffCount', () => {
	it('counts added, removed and changed keys', () => {
		expect(diffCount({ a: 1, b: 2 }, { a: 1, b: 2 })).toBe(0);
		expect(diffCount({ a: 1 }, { a: 2 })).toBe(1);
		expect(diffCount({ a: 1, b: 1 }, { a: 1 })).toBe(1);
		expect(diffCount({}, { a: 1, b: 1 })).toBe(2);
	});

	it('treats undefined and null as the same absence', () => {
		expect(diffCount({ a: null }, {})).toBe(0);
	});
});

describe('softFlat', () => {
	it('flattens a cap and a bypass into separate entries', () => {
		expect(softFlat({ session: { cap: 80, capUsd: null, bypass: 30 } })).toEqual({
			'session.cap': 80,
			'session.bypass': 30
		});
	});

	it('drops empty windows', () => {
		expect(softFlat({ weekly_all: { cap: null, capUsd: null, bypass: null } })).toEqual({});
	});
});

describe('settingsSlice', () => {
	it('keeps only the keys and env vars of that page', () => {
		const settings = { a: true, b: false, env: { FOO: '1', BAR: '2' } };
		expect(settingsSlice(settings, ['a'], ['FOO'])).toEqual({ a: true, 'env.FOO': '1' });
	});

	it('ignores a non-object env blob', () => {
		expect(settingsSlice({ env: 'nope' }, [], ['FOO'])).toEqual({});
	});
});

describe('looseSettings', () => {
	it('returns the entries no page claims', () => {
		const settings = { model: 'x', editorMode: 'vim', env: { A: '1' } };
		expect(looseSettings(settings, new Set(['model']))).toEqual([['editorMode', 'vim']]);
	});
});
