import { describe, expect, it } from 'vitest';
import {
	adapterMachines,
	binaryStatus,
	formatAdapterConfig,
	parseAdapterConfig
} from './machineAdapters.logic';
import type { MachineResourcesRow } from '@bindings/MachineResourcesRow';
import type { MachineRow } from '@bindings/MachineRow';
import type { MachineAdapterInfo } from '@bindings/MachineAdapterInfo';

const adapter = (over: Partial<MachineAdapterInfo> = {}): MachineAdapterInfo => ({
	adapter_id: 'codex',
	enabled: true,
	config: {},
	pinned: false,
	default_enabled: true,
	...over
});

describe('binaryStatus', () => {
	it('is unknown until the daemon reports a probe', () => {
		expect(binaryStatus(adapter())).toBe('unknown');
	});

	it('is found with a resolved path and missing without one', () => {
		const found = adapter({
			binary: { bin: 'codex', config_key: 'codex_bin', path: '/usr/bin/codex' }
		});
		expect(binaryStatus(found)).toBe('found');
		expect(binaryStatus(adapter({ binary: { bin: 'codex', config_key: 'codex_bin' } }))).toBe(
			'missing'
		);
	});
});

describe('parseAdapterConfig', () => {
	it('accepts a JSON object and treats a blank editor as {}', () => {
		expect(parseAdapterConfig('{"bin": "/opt/opencode", "env": {"A": "1"}}')).toEqual({
			ok: true,
			config: { bin: '/opt/opencode', env: { A: '1' } }
		});
		expect(parseAdapterConfig('  \n')).toEqual({ ok: true, config: {} });
	});

	it('reports a syntax error with the parser detail', () => {
		const got = parseAdapterConfig('{"bin": }');
		expect(got.ok).toBe(false);
		expect(got.ok === false && got.reason).toBe('syntax');
		expect(got.ok === false && got.reason === 'syntax' && got.detail.length > 0).toBe(true);
	});

	it('refuses JSON that is not an object', () => {
		for (const text of ['[]', 'null', '"codex"', '42']) {
			expect(parseAdapterConfig(text)).toEqual({ ok: false, reason: 'not_object' });
		}
	});
});

describe('formatAdapterConfig', () => {
	it('round-trips through the parser', () => {
		const config = { codex_bin: '/opt/codex', args: ['--x'] };
		expect(parseAdapterConfig(formatAdapterConfig(config))).toEqual({ ok: true, config });
	});
});

describe('adapterMachines', () => {
	const all = [
		{ id: 'a', name: 'alpha', display_name: 'Alpha' },
		{ id: 'b', name: 'beta', display_name: null }
	] as MachineRow[];
	const own = [{ machine_id: 'm', name: 'mine', display_name: null }] as MachineResourcesRow[];

	it('an admin gets the full machine list', () => {
		expect(adapterMachines(true, all, own)).toEqual([
			{ id: 'a', name: 'Alpha' },
			{ id: 'b', name: 'beta' }
		]);
	});

	it('a non-admin gets only their own machines', () => {
		expect(adapterMachines(false, all, own)).toEqual([{ id: 'm', name: 'mine' }]);
	});

	it('is empty while the source query has no data', () => {
		expect(adapterMachines(true, undefined, own)).toEqual([]);
		expect(adapterMachines(false, all, undefined)).toEqual([]);
	});
});
