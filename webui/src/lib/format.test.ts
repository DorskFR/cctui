import { describe, expect, it } from 'vitest';
import { machineInitial, modelAbbrev, modelFamily, modelShort, usd } from './format';

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

describe('modelAbbrev', () => {
	it('abbreviates the Claude families', () => {
		expect(modelAbbrev('claude-opus-4-8')).toBe('Op.');
		expect(modelAbbrev('claude-sonnet-5')).toBe('So.');
		expect(modelAbbrev('claude-fable-5-1')).toBe('Fa.');
		expect(modelAbbrev('claude-haiku-4-5-20251001')).toBe('Ha.');
	});

	it('tells the Codex codenames apart, where the family word cannot', () => {
		expect(modelFamily('gpt-5.6-sol')).toBe('gpt');
		expect(modelFamily('gpt-5.6-terra')).toBe('gpt');
		expect(modelAbbrev('gpt-5.6-sol')).toBe('So.');
		expect(modelAbbrev('gpt-5.6-terra')).toBe('Te.');
		expect(modelAbbrev('gpt-5.6-luna')).toBe('Lu.');
		expect(modelAbbrev('gpt-6-astra')).toBe('As.');
	});

	it('does not read a codename out of the middle of a word', () => {
		expect(modelAbbrev('gpt-5-solaris-preview')).toBe('GPT');
		expect(modelAbbrev('claude-opus-4-8-consolidated')).toBe('Op.');
	});

	it('falls back to the first two letters of an unknown engine', () => {
		expect(modelAbbrev('fireworks-ai/accounts/fireworks/models/kimi-k3')).toBe('Ki.');
	});
});

describe('machineInitial', () => {
	it('keeps the trailing number, so a numbered fleet stays distinguishable', () => {
		expect(machineInitial('workstation-01')).toBe('W1');
		expect(machineInitial('ci-runner-02')).toBe('C2');
		expect(machineInitial('ci-runner-11')).toBe('C11');
		expect(machineInitial('dev1')).toBe('D1');
	});

	it('is a single letter when there is no number to keep', () => {
		expect(machineInitial('nanachi')).toBe('N');
		expect(machineInitial('build farm')).toBe('B');
	});

	it('survives the id fallback and an empty name', () => {
		expect(machineInitial('a0000000')).toBe('A0');
		expect(machineInitial('')).toBe('?');
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
