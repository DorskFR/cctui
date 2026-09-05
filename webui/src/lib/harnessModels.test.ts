import { describe, expect, it } from 'vitest';
import type { CodexModelCatalog } from '@bindings/CodexModelCatalog';
import {
	OTHER_MODEL,
	codexModels,
	codexModelsFor,
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
		expect(withCurrentModel(codexModels, codexModels[1].v)).toBe(codexModels);
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
