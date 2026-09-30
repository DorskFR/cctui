import { describe, expect, it } from 'vitest';
import type { ModelOption } from '@bindings/ModelOption';
import {
	DEFAULT_MODELS,
	OTHER_MODEL,
	customModelValue,
	declaredModelOptions,
	modelHintText,
	withCurrentModel,
	withDeclaredModels
} from './harnessModels';

const option = (v: string, label: string): ModelOption => ({ v, label, disabled: false });

describe('customModelValue', () => {
	it('trims and treats blank as default', () => {
		expect(customModelValue('  gpt-6-astra ')).toBe('gpt-6-astra');
		expect(customModelValue('   ')).toBe('');
	});
});

describe('withCurrentModel', () => {
	it('lists an unknown current id as its own option', () => {
		expect(withCurrentModel(DEFAULT_MODELS, 'gpt-nonesuch').at(-1)).toEqual({
			v: 'gpt-nonesuch',
			label: 'gpt-nonesuch',
			disabled: false
		});
	});

	it('leaves the list alone for a known or empty value', () => {
		expect(withCurrentModel(DEFAULT_MODELS, '')).toBe(DEFAULT_MODELS);
		expect(withCurrentModel(DEFAULT_MODELS, DEFAULT_MODELS[0].v)).toBe(DEFAULT_MODELS);
	});

	it('never mistakes the sentinel for a model', () => {
		expect(DEFAULT_MODELS.some((o) => o.v === OTHER_MODEL)).toBe(false);
	});
});

describe('modelHintText', () => {
	it('words the server codes and says nothing without one', () => {
		expect(modelHintText(undefined)).toBeUndefined();
		expect(modelHintText({ kind: 'needs_version', version: '0.153.0' })).toContain('0.153.0');
		const gated = modelHintText({ kind: 'gated', version: '0.999.0', current: '0.156.1' });
		expect(gated).toContain('0.999.0');
		expect(gated).toContain('0.156.1');
	});
});

describe('withDeclaredModels', () => {
	const claude = [option('', 'Default'), option('opus', 'Opus')];

	it('offers every declared model, then what the fallback adds', () => {
		const declared = [
			{ model: 'claude-opus-4-8', label: 'Opus 4.8' },
			{ model: 'claude-opus-5', label: 'Opus 5' }
		];
		expect(withDeclaredModels(declared, claude)).toEqual([
			option('', 'Default'),
			option('claude-opus-4-8', 'Opus 4.8'),
			option('claude-opus-5', 'Opus 5'),
			option('opus', 'Opus')
		]);
	});

	it('restores the plain list when the declared list is empty', () => {
		expect(withDeclaredModels([], claude)).toBe(claude);
		expect(withDeclaredModels(null, claude)).toBe(claude);
		expect(withDeclaredModels([{ model: '  ', label: 'blank row' }], claude)).toBe(claude);
	});

	it('never lists a declared id twice', () => {
		const out = withDeclaredModels([{ model: 'opus', label: 'Opus (pinned)' }], claude);
		expect(out.filter((o) => o.v === 'opus')).toEqual([option('opus', 'Opus (pinned)')]);
	});
});

describe('declaredModelOptions', () => {
	it('falls back to the id as the label and drops half-filled rows', () => {
		expect(
			declaredModelOptions([
				{ model: ' claude-opus-5 ', label: '' },
				{ model: '', label: 'nothing' }
			])
		).toEqual([option('claude-opus-5', 'claude-opus-5')]);
	});
});
