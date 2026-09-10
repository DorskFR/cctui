import { describe, expect, it } from 'vitest';
import type { CodexDiagnose } from '@bindings/CodexDiagnose';
import type { CodexProtocolError } from '@bindings/CodexProtocolError';
import type { CodexRpcFrame } from '@bindings/CodexRpcFrame';
import { silenceReasons } from './diagnoseSilence';

const NOW = 1_700_000_000_000;

function frame(ts_ms: number, transport: string): CodexRpcFrame {
	return { ts_ms, direction: 'out', label: 'thread/list', json: '{}', transport };
}

function protocolError(ts_ms: number, transport: string, message: string): CodexProtocolError {
	return { ts_ms, message, transport };
}

function diagnose(over: Partial<CodexDiagnose> = {}): CodexDiagnose {
	return {
		min_version: '0.153.0',
		transport: 'stdio',
		live: true,
		registered: true,
		active_turn_id: 'turn-1',
		turn_status: 'working',
		pending_rpc_count: 0,
		pending_rpc_methods: [],
		protocol_errors: [],
		stderr_tail: [],
		rpc_tail: [frame(NOW - 1_000, 'stdio'), frame(NOW - 500, 'shared')],
		auth_state: 'gateway env present',
		...over
	} as CodexDiagnose;
}

describe('silenceReasons', () => {
	it('reports nothing when the session is merely busy and healthy', () => {
		expect(silenceReasons(diagnose(), NOW)).toEqual([]);
	});

	it('flags an RPC outstanding with no frame for over a minute', () => {
		const reasons = silenceReasons(
			diagnose({ pending_rpc_count: 2, rpc_tail: [frame(NOW - 120_000, 'shared')] }),
			NOW
		);
		expect(reasons.some((r) => r.includes('2') && r.includes('outstanding'))).toBe(true);
	});

	it('does not flag a pending RPC that is still young', () => {
		const reasons = silenceReasons(
			diagnose({ pending_rpc_count: 2, rpc_tail: [frame(NOW - 5_000, 'shared')] }),
			NOW
		);
		expect(reasons.some((r) => r.includes('outstanding'))).toBe(false);
	});

	it('surfaces requests failed by a shared-connection drop', () => {
		const reasons = silenceReasons(
			diagnose({
				protocol_errors: [
					protocolError(NOW - 2_000, 'shared', 'connection dropped before request 7 was answered')
				]
			}),
			NOW
		);
		expect(reasons.some((r) => r.includes('shared app-server connection dropped'))).toBe(true);
	});

	it('ignores a stdio protocol error when looking for shared-connection drops', () => {
		const reasons = silenceReasons(
			diagnose({ protocol_errors: [protocolError(NOW - 2_000, 'stdio', 'turn/start: boom')] }),
			NOW
		);
		expect(reasons.some((r) => r.includes('shared app-server connection dropped'))).toBe(false);
	});

	it('flags traffic on stdio with none on the shared connection', () => {
		const reasons = silenceReasons(diagnose({ rpc_tail: [frame(NOW - 500, 'stdio')] }), NOW);
		expect(reasons.some((r) => r.includes('No JSON-RPC frames on the shared'))).toBe(true);
	});

	it('does not claim a shared blind spot when there is no traffic at all', () => {
		const reasons = silenceReasons(diagnose({ rpc_tail: [] }), NOW);
		expect(reasons.some((r) => r.includes('No JSON-RPC frames on the shared'))).toBe(false);
	});

	it('reports no turn, bad auth, registry mismatch and a dead child', () => {
		const reasons = silenceReasons(
			diagnose({
				active_turn_id: null,
				auth_state: 'no gateway env',
				registry_live_mismatch: 'registered but not live',
				live: false
			}),
			NOW
		);
		expect(reasons).toHaveLength(4);
		expect(reasons.some((r) => r.includes('No turn'))).toBe(true);
		expect(reasons.some((r) => r.includes('no gateway env'))).toBe(true);
		expect(reasons.some((r) => r.includes('registered but not live'))).toBe(true);
		expect(reasons.some((r) => r.includes('No live app-server child'))).toBe(true);
	});
});
