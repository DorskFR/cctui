import { describe, expect, it } from 'vitest';
import { binaryStatus, formatAdapterConfig, parseAdapterConfig } from './machineAdapters.logic';
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
