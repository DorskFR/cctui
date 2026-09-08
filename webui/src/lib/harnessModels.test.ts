import { describe, expect, it } from 'vitest';
import type { CodexModelCatalog } from '@bindings/CodexModelCatalog';
import {
	OTHER_MODEL,
	codexModels,
	codexModelsFor,
	declaredModelOptions,
	withDeclaredModels,
	customModelValue,
	preferCatalog,
	withCurrentModel
} from './harnessModels';

const catalog = (...ids: string[]): CodexModelCatalog => ({
	models: ids.map((id) => ({
		id,
		model: id,
		display_name: id.toUpperCase(),
		description: '',
		hidden: false,
		is_default: false,
		supported_efforts: [],
		default_effort: '',
		input_modalities: []
	}))
});

describe('customModelValue', () => {
	it('trims and treats blank as default', () => {
		expect(customModelValue('  gpt-6-astra ')).toBe('gpt-6-astra');
		expect(customModelValue('   ')).toBe('');
	});
});

describe('withCurrentModel', () => {
	it('lists an unknown current id as its own option', () => {
		// Deliberately an id the static list does not carry: the point is the
		// fallback path for a remembered/free-text model, not this id.
		expect(withCurrentModel(codexModels, 'gpt-nonesuch').at(-1)).toEqual({
			v: 'gpt-nonesuch',
			label: 'gpt-nonesuch'
		});
	});

	it('leaves the list alone for a known or empty value', () => {
		expect(withCurrentModel(codexModels, '')).toBe(codexModels);
		expect(withCurrentModel(codexModels, codexModels[0].v)).toBe(codexModels);
	});

	it('never mistakes the sentinel for a model', () => {
		expect(codexModels.some((o) => o.v === OTHER_MODEL)).toBe(false);
	});
});

describe('preferCatalog', () => {
	it('takes the first non-empty catalog', () => {
		const merged = catalog('gpt-b');
		expect(preferCatalog(undefined, { models: [] }, merged)).toBe(merged);
		expect(preferCatalog(undefined, undefined)).toBeUndefined();
	});

	it('drives the picker, static list only when nothing is live', () => {
		expect(codexModelsFor(preferCatalog(catalog('gpt-a'), catalog('gpt-b'))).map((o) => o.v)).toEqual(['', 'gpt-a']);
		expect(codexModelsFor(preferCatalog(undefined))).toBe(codexModels);
	});
});

describe('codexModels', () => {
	it('hardcodes no model slug', () => {
		expect(codexModels.map((o) => o.v)).toEqual(['']);
	});
});

describe('withDeclaredModels', () => {
	const claude = [
		{ v: '', label: 'Default' },
		{ v: 'opus', label: 'Opus' }
	];

	it('offers every declared model, then what the fallback adds', () => {
		const declared = [
			{ model: 'claude-opus-4-8', label: 'Opus 4.8' },
			{ model: 'claude-opus-5', label: 'Opus 5' }
		];
		expect(withDeclaredModels(declared, claude)).toEqual([
			{ v: '', label: 'Default' },
			{ v: 'claude-opus-4-8', label: 'Opus 4.8' },
			{ v: 'claude-opus-5', label: 'Opus 5' },
			{ v: 'opus', label: 'Opus' }
		]);
	});

	it('restores the plain list when the declared list is empty', () => {
		expect(withDeclaredModels([], claude)).toBe(claude);
		expect(withDeclaredModels(null, claude)).toBe(claude);
		expect(withDeclaredModels([{ model: '  ', label: 'blank row' }], claude)).toBe(claude);
	});

	it('never lists a declared id twice', () => {
		const out = withDeclaredModels([{ model: 'opus', label: 'Opus (pinned)' }], claude);
		expect(out.filter((o) => o.v === 'opus')).toEqual([{ v: 'opus', label: 'Opus (pinned)' }]);
	});
});

describe('declaredModelOptions', () => {
	it('falls back to the id as the label and drops half-filled rows', () => {
		expect(
			declaredModelOptions([
				{ model: ' claude-opus-5 ', label: '' },
				{ model: '', label: 'nothing' }
			])
		).toEqual([{ v: 'claude-opus-5', label: 'claude-opus-5' }]);
	});
});
