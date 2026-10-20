import { describe, expect, it } from 'vitest';
import type { HarnessDescriptor } from '@bindings/HarnessDescriptor';
import {
	NO_CAPABILITIES,
	brandMark,
	familyAccent,
	harnessCapabilities,
	harnessForAdapter,
	harnessForProvider,
	harnessLabel,
	harnessPermissionModes,
	harnessesForProviders,
	pickableHarnesses
} from '$lib/harnesses';
import { harnessModelsFallback } from '$lib/domainTables';
import { adapterLabel, allAdapters, modesFor } from '$lib/components/organisms/spawn/options';
import { parityFixture } from '$lib/parity/fixtures';

const table = parityFixture<{ harnesses: HarnessDescriptor[] }>('domainTables').harnesses;

describe('harness picker options', () => {
	it('lists every default-enabled row in table order, opencode included', () => {
		expect(pickableHarnesses(table).map((h) => h.id)).toEqual(['claude-code', 'codex', 'opencode']);
		expect(allAdapters).toEqual(['claude-code', 'codex', 'opencode']);
	});

	it('narrows to the ids a machine reports as enabled', () => {
		expect(pickableHarnesses(table, ['codex']).map((h) => h.id)).toEqual(['codex']);
		expect(pickableHarnesses(table, [])).toEqual([]);
	});

	it('ships the ACP agents default-off, listed only where a machine enables them', () => {
		const ids = table.map((h) => h.id);
		expect(ids).toEqual(['claude-code', 'codex', 'opencode', 'gemini', 'qwen', 'goose']);
		expect(pickableHarnesses(table, ['goose', 'codex']).map((h) => h.id)).toEqual([
			'codex',
			'goose'
		]);
		for (const acp of ['gemini', 'qwen', 'goose']) {
			expect(harnessPermissionModes(acp, table)).toEqual(['ask', 'auto', 'yolo']);
			expect(harnessCapabilities(acp, table).resume).toBe(true);
			expect(harnessCapabilities(acp, table).fork).toBe(false);
		}
	});

	it('leaves a default-off row out until a machine enables it', () => {
		const kilo: HarnessDescriptor = {
			...table[0],
			id: 'kilo',
			default_enabled: false
		};
		const withKilo = [...table, kilo];
		expect(pickableHarnesses(withKilo).map((h) => h.id)).not.toContain('kilo');
		expect(pickableHarnesses(withKilo, ['kilo']).map((h) => h.id)).toEqual(['kilo']);
		expect(pickableHarnesses(table).map((h) => h.id)).not.toContain('gemini');
	});

	it('labels come from the descriptor and an unknown id stays itself', () => {
		for (const h of table) expect(harnessLabel(h.id, table)).toBe(h.label);
		expect(adapterLabel('gemini')).toBe('Gemini CLI');
		expect(adapterLabel('kilo')).toBe('kilo');
	});
});

describe('capability gates', () => {
	it('derive from the descriptor row, variants included', () => {
		for (const h of table) {
			expect(harnessCapabilities(h.id, table)).toEqual(h.capabilities);
			expect(harnessCapabilities(`${h.id}-variant`, table)).toEqual(h.capabilities);
		}
		expect(harnessForAdapter('codex-app-server', table)?.id).toBe('codex');
	});

	it('only claude forks and only codex switches model in place', () => {
		expect(harnessCapabilities('claude-code', table).fork).toBe(true);
		expect(harnessCapabilities('codex', table).fork).toBe(false);
		expect(harnessCapabilities('codex', table).set_model).toBe(true);
		expect(harnessCapabilities('claude-code', table).set_model).toBe(false);
		for (const h of table) expect(harnessCapabilities(h.id, table).mid_chat_files).toBe(true);
	});

	it('an unknown id can do nothing', () => {
		expect(harnessCapabilities('kilo', table)).toEqual(NO_CAPABILITIES);
		expect(harnessCapabilities(null, table)).toEqual(NO_CAPABILITIES);
		expect(harnessForAdapter('codexx', table)).toBeUndefined();
		expect(harnessForAdapter('claude', table)).toBeUndefined();
	});
});

describe('brand marks', () => {
	it('follow the harness family for adapters and the provider table for providers', () => {
		expect(brandMark({ adapter: 'claude-code' }, table)).toBe('anthropic');
		expect(brandMark({ adapter: 'codex-app-server' }, table)).toBe('openai');
		expect(brandMark({ adapter: 'opencode' }, table)).toBe('fireworks');
		expect(brandMark({ provider: 'openai-compatible' }, table)).toBe('openai');
		expect(brandMark({ provider: 'fireworks' }, table)).toBe('fireworks');
		expect(brandMark({ provider: 'google' }, table)).toBe('google');
	});

	it('an ACP agent with a mark of its own wears it, not the OpenAI blossom', () => {
		expect(brandMark({ adapter: 'qwen' }, table)).toBe('qwen');
		expect(brandMark({ adapter: 'goose-acp' }, table)).toBe('goose');
		expect(familyAccent('qwen')).not.toBe(familyAccent('neutral'));
		expect(familyAccent('goose')).not.toBe(familyAccent('neutral'));
	});

	it('an unknown id or no id wears the neutral glyph, never the claude logo', () => {
		expect(brandMark({ adapter: 'gemini' }, table)).toBe('google');
		expect(brandMark({ adapter: 'kilo' }, table)).toBe('neutral');
		expect(brandMark({ adapter: null }, table)).toBe('neutral');
		expect(brandMark({}, table)).toBe('neutral');
		expect(brandMark({ provider: 'nonesuch' }, table)).toBe('neutral');
		expect(familyAccent('neutral')).not.toBe(familyAccent('anthropic'));
	});
});

describe('models for an unknown harness', () => {
	it('offer only the default, no claude models', () => {
		expect(harnessModelsFallback('kilo').models.map((o) => o.v)).toEqual(['']);
	});
});

describe('permission modes', () => {
	it('offer exactly what the descriptor lists', () => {
		const narrow: HarnessDescriptor = {
			...table[1],
			id: 'narrow',
			permission_modes: ['ask', 'yolo']
		};
		expect(harnessPermissionModes('narrow', [...table, narrow])).toEqual(['ask', 'yolo']);
		for (const h of table) expect(harnessPermissionModes(h.id, table)).toEqual(h.permission_modes);
		expect(modesFor('codex').map((md) => md.v)).toEqual(['ask', 'auto', 'yolo', 'whip']);
	});

	it('keep the full list for a harness the table does not know', () => {
		expect(harnessPermissionModes('kilo', table)).toEqual(['ask', 'auto', 'yolo', 'whip']);
	});
});

describe('provider to harness', () => {
	it('is the first row of the credential family', () => {
		expect(harnessForProvider('anthropic-compatible', table)).toBe('claude-code');
		expect(harnessForProvider('openai', table)).toBe('codex');
		expect(harnessForProvider('fireworks', table)).toBe('opencode');
		expect(harnessesForProviders(['fireworks', 'anthropic'], table)).toEqual([
			'claude-code',
			'opencode'
		]);
	});
});
