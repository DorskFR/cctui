import { describe, expect, it } from 'vitest';
import { deniedHealth, versionSkew, DRAIN_RESTARTABLE } from './harnessHealth';

describe('versionSkew', () => {
	it('names only a daemon running a different version than its CLI', () => {
		expect(
			versionSkew({
				'claude-code': { cli: '2.1.286', daemon: '2.1.285' },
				codex: { cli: '0.155.0', daemon: '0.155.0' },
				gemini: { cli: '0.62.0' }
			})
		).toEqual([{ harness: 'claude-code', cli: '2.1.286', daemon: '2.1.285' }]);
		expect(versionSkew({})).toEqual([]);
		expect(versionSkew({ codex: { daemon: '1' } })).toEqual([]);
	});
});

describe('deniedHealth', () => {
	it('is null when healthy or never probed, the denial otherwise', () => {
		expect(deniedHealth(null)).toBeNull();
		expect(deniedHealth({ health: null })).toBeNull();
		expect(deniedHealth({})).toBeNull();
		expect(deniedHealth({ health: { state: 'ok' } })).toBeNull();
		expect(deniedHealth({ health: { state: 'network_denied', detail: 'connect: EHOSTUNREACH' } })).toEqual({
			state: 'network_denied',
			detail: 'connect: EHOSTUNREACH'
		});
		expect(deniedHealth({ health: { state: 'folder_denied', detail: 'list ~/Documents: EPERM' } })?.state).toBe(
			'folder_denied'
		);
	});

	it('only claude-code offers a drain restart', () => {
		expect(DRAIN_RESTARTABLE.has('claude-code')).toBe(true);
		expect(DRAIN_RESTARTABLE.has('codex')).toBe(false);
	});
});
