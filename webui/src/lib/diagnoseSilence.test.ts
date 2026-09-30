import { describe, expect, it } from 'vitest';
import type { SilenceReason } from '@bindings/SilenceReason';
import { fmtAge, silenceMessages, silenceReasonMessage } from './diagnoseSilence';

describe('silenceReasonMessage', () => {
	it('words every code the server can send', () => {
		const reasons: SilenceReason[] = [
			{ kind: 'codex_stalled_rpc', count: 2, age_ms: 120_000 },
			{ kind: 'codex_shared_dropped', count: 1, age_ms: 2_000 },
			{ kind: 'codex_shared_no_frames' },
			{ kind: 'codex_no_turn' },
			{ kind: 'codex_auth', state: 'no gateway env' },
			{ kind: 'codex_registry_mismatch', detail: 'registered but not live' },
			{ kind: 'codex_not_live' },
			{ kind: 'opencode_sse_down' },
			{ kind: 'opencode_sse_stalled', age_ms: 120_000 },
			{ kind: 'opencode_sse_no_frames' },
			{ kind: 'opencode_http_errors', count: 1, age_ms: 2_000, message: 'POST /prompt: 500' },
			{ kind: 'opencode_awaiting_permission', count: 2 },
			{ kind: 'opencode_idle' },
			{ kind: 'opencode_version', version: '1.20.0', pinned: '1.18.7' },
			{ kind: 'opencode_not_live' }
		];
		for (const reason of reasons) expect(silenceReasonMessage(reason)).toBeTruthy();
	});

	it('carries the numbers the code supplies into the sentence', () => {
		const stalled = silenceReasonMessage({
			kind: 'codex_stalled_rpc',
			count: 2,
			age_ms: 120_000
		});
		expect(stalled).toContain('2');
		expect(stalled).toContain(fmtAge(120_000));
		expect(
			silenceReasonMessage({
				kind: 'opencode_http_errors',
				count: 1,
				age_ms: 2_000,
				message: 'POST /prompt: 500'
			})
		).toContain('500');
		expect(
			silenceReasonMessage({ kind: 'opencode_version', version: '1.20.0', pinned: '1.18.7' })
		).toContain('1.20.0');
	});
});

describe('silenceMessages', () => {
	it('keeps each harness to its own section', () => {
		const reasons: SilenceReason[] = [
			{ kind: 'codex_no_turn' },
			{ kind: 'opencode_idle' },
			{ kind: 'opencode_not_live' }
		];
		expect(silenceMessages(reasons, 'codex')).toHaveLength(1);
		expect(silenceMessages(reasons, 'opencode')).toHaveLength(2);
		expect(silenceMessages([], 'codex')).toEqual([]);
	});
});
