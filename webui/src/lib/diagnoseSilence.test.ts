import { describe, expect, it } from 'vitest';
import type { CodexDiagnose } from '@bindings/CodexDiagnose';
import type { TrafficError } from '@bindings/TrafficError';
import type { TrafficFrame } from '@bindings/TrafficFrame';
import type { OpenCodeDiagnose } from '@bindings/OpenCodeDiagnose';
import { openCodeSilenceReasons, silenceReasons } from './diagnoseSilence';

const NOW = 1_700_000_000_000;

function frame(ts_ms: number, transport: string): TrafficFrame {
	return { ts_ms, direction: 'out', label: 'thread/list', json: '{}', transport };
}

function protocolError(ts_ms: number, transport: string, message: string): TrafficError {
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

describe('openCodeSilenceReasons', () => {
	function oc(over: Partial<OpenCodeDiagnose> = {}): OpenCodeDiagnose {
		return {
			pinned_version: '1.18.7',
			server_version: '1.18.7',
			version_matches: true,
			live: true,
			owned_sessions: ['ses_1'],
			turn_status: 'working',
			sse_connected: true,
			last_sse_event_ms: NOW - 1_000,
			pending_permissions: [],
			protocol_errors: [],
			stderr_tail: [],
			rpc_tail: [frame(NOW - 2_000, 'http'), frame(NOW - 1_000, 'sse')],
			...over
		} as OpenCodeDiagnose;
	}

	it('reports nothing when the session is merely busy and healthy', () => {
		expect(openCodeSilenceReasons(oc(), NOW)).toEqual([]);
	});

	it('flags a disconnected event stream', () => {
		const reasons = openCodeSilenceReasons(oc({ sse_connected: false }), NOW);
		expect(reasons.some((r) => r.includes('event stream is not connected'))).toBe(true);
	});

	it('flags a turn in flight with no event for over a minute', () => {
		const reasons = openCodeSilenceReasons(oc({ last_sse_event_ms: NOW - 120_000 }), NOW);
		expect(reasons.some((r) => r.includes('no event has arrived'))).toBe(true);
	});

	it('does not flag an event gap that is still young', () => {
		const reasons = openCodeSilenceReasons(oc({ last_sse_event_ms: NOW - 5_000 }), NOW);
		expect(reasons.some((r) => r.includes('no event has arrived'))).toBe(false);
	});

	it('names the event path as the blind spot when only HTTP frames exist', () => {
		const reasons = openCodeSilenceReasons(
			oc({ rpc_tail: [frame(NOW - 1_000, 'http')], last_sse_event_ms: null }),
			NOW
		);
		expect(reasons.some((r) => r.includes('GET /event'))).toBe(true);
	});

	it('does not claim an event blind spot when there is no traffic at all', () => {
		const reasons = openCodeSilenceReasons(oc({ rpc_tail: [], last_sse_event_ms: null }), NOW);
		expect(reasons.some((r) => r.includes('GET /event'))).toBe(false);
	});

	it('surfaces rejected HTTP calls and ignores SSE errors when doing so', () => {
		const reasons = openCodeSilenceReasons(
			oc({
				protocol_errors: [
					protocolError(NOW - 3_000, 'sse', 'event stream closed'),
					protocolError(NOW - 2_000, 'http', 'POST /session/ses_1/prompt_async: 500')
				]
			}),
			NOW
		);
		const rejected = reasons.filter((r) => r.includes('rejected'));
		expect(rejected).toHaveLength(1);
		expect(rejected[0]).toContain('1 call(s)');
		expect(rejected[0]).toContain('500');
	});

	it('reports a pending permission, an idle turn, a version drift and a dead server', () => {
		const reasons = openCodeSilenceReasons(
			oc({
				pending_permissions: ['perm_1', 'perm_2'],
				turn_status: 'idle',
				server_version: '1.20.0',
				version_matches: false,
				live: false
			}),
			NOW
		);
		expect(reasons).toHaveLength(4);
		expect(reasons.some((r) => r.includes('2 permission prompt(s)'))).toBe(true);
		expect(reasons.some((r) => r.includes('No turn'))).toBe(true);
		expect(reasons.some((r) => r.includes('1.20.0'))).toBe(true);
		expect(reasons.some((r) => r.includes('No live `opencode serve`'))).toBe(true);
	});
});
