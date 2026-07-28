import { describe, expect, it } from 'vitest';
import { modelFamily, modelShort, usd } from './format';

describe('modelShort', () => {
	it('drops the vendor prefix', () => {
		expect(modelShort('claude-opus-4-8')).toBe('opus-4-8');
		expect(modelShort('gpt-5-codex')).toBe('gpt-5-codex');
	});

	it('keeps only the leaf of a path-qualified provider id', () => {
		expect(modelShort('fireworks-ai/accounts/fireworks/models/kimi-k3')).toBe('kimi-k3');
		expect(modelShort('accounts/fireworks/models/kimi-k3')).toBe('kimi-k3');
	});

	it('survives a trailing slash', () => {
		expect(modelShort('fireworks-ai/')).toBe('fireworks-ai');
	});
});

describe('modelFamily', () => {
	it('reduces a qualified id to its family word', () => {
		expect(modelFamily('fireworks-ai/accounts/fireworks/models/kimi-k3')).toBe('kimi');
		expect(modelFamily('claude-opus-4-8')).toBe('opus');
	});
});

describe('usd', () => {
	it('keeps sub-cent spend visible', () => {
		expect(usd(0.0031)).toBe('$0.0031');
	});

	it('renders ordinary amounts to the cent', () => {
		expect(usd(0.71)).toBe('$0.71');
		expect(usd(12)).toBe('$12.00');
	});

	it('drops cents once the amount is large', () => {
		expect(usd(1234.56)).toBe('$1235');
	});

	it('floors at zero for missing or negative input', () => {
		expect(usd(0)).toBe('$0.00');
		expect(usd(Number.NaN)).toBe('$0.00');
	});
});
