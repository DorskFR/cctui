import type { SilenceReason } from '@bindings/SilenceReason';
import { m } from '$lib/paraglide/messages';

export function fmtAge(ms: number | null): string {
	if (ms === null) return m.diagnose_undated();
	if (ms < 1_000) return m.diagnose_age_ms({ ms });
	if (ms < 60_000) return m.diagnose_age_s({ s: Math.floor(ms / 1_000) });
	if (ms < 3_600_000) return m.diagnose_age_m({ min: Math.floor(ms / 60_000) });
	return m.diagnose_age_h({ h: Math.floor(ms / 3_600_000) });
}

/** The server decides which reasons apply; this only words them. */
export function silenceReasonMessage(reason: SilenceReason): string {
	switch (reason.kind) {
		case 'codex_stalled_rpc':
			return m.diagnose_codex_silence_stalled_rpc({
				count: reason.count,
				age: fmtAge(reason.age_ms)
			});
		case 'codex_shared_dropped':
			return m.diagnose_codex_silence_shared_dropped({
				count: reason.count,
				age: fmtAge(reason.age_ms)
			});
		case 'codex_shared_no_frames':
			return m.diagnose_codex_silence_shared_no_frames();
		case 'codex_no_turn':
			return m.diagnose_codex_silence_no_turn();
		case 'codex_auth':
			return m.diagnose_codex_silence_auth({ state: reason.state });
		case 'codex_registry_mismatch':
			return m.diagnose_codex_silence_mismatch({ detail: reason.detail });
		case 'codex_not_live':
			return m.diagnose_codex_silence_not_live();
		case 'opencode_sse_down':
			return m.diagnose_opencode_silence_sse_down();
		case 'opencode_sse_stalled':
			return m.diagnose_opencode_silence_sse_stalled({ age: fmtAge(reason.age_ms) });
		case 'opencode_sse_no_frames':
			return m.diagnose_opencode_silence_sse_no_frames();
		case 'opencode_http_errors':
			return m.diagnose_opencode_silence_http_errors({
				count: reason.count,
				age: fmtAge(reason.age_ms),
				message: reason.message
			});
		case 'opencode_awaiting_permission':
			return m.diagnose_opencode_silence_awaiting_permission({ count: reason.count });
		case 'opencode_idle':
			return m.diagnose_opencode_silence_idle();
		case 'opencode_version':
			return m.diagnose_opencode_silence_version({
				version: reason.version,
				pinned: reason.pinned
			});
		case 'opencode_not_live':
			return m.diagnose_opencode_silence_not_live();
	}
}

/** Codex reasons keep their own section, opencode reasons theirs. */
export function silenceMessages(reasons: SilenceReason[], harness: 'codex' | 'opencode'): string[] {
	return reasons
		.filter((r) => r.kind.startsWith(harness === 'codex' ? 'codex_' : 'opencode_'))
		.map(silenceReasonMessage);
}
