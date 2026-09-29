import type { CodexDiagnose } from '@bindings/CodexDiagnose';
import type { OpenCodeDiagnose } from '@bindings/OpenCodeDiagnose';
import { m } from '$lib/paraglide/messages';

const STALLED_RPC_MS = 60_000;

export function fmtAge(ms: number | null): string {
	if (ms === null) return m.diagnose_undated();
	if (ms < 1_000) return m.diagnose_age_ms({ ms });
	if (ms < 60_000) return m.diagnose_age_s({ s: Math.floor(ms / 1_000) });
	if (ms < 3_600_000) return m.diagnose_age_m({ min: Math.floor(ms / 60_000) });
	return m.diagnose_age_h({ h: Math.floor(ms / 3_600_000) });
}

/** Each entry is an independent reason the session can look silent, derived
 * from the codex facts alone with no extra sensing. */
export function silenceReasons(cx: CodexDiagnose, generatedAtMs: number): string[] {
	const out: string[] = [];
	const frames = cx.rpc_tail ?? [];
	const lastFrameMs = frames.length ? frames[frames.length - 1].ts_ms : null;
	const idleMs = lastFrameMs === null ? null : generatedAtMs - lastFrameMs;
	if (cx.pending_rpc_count > 0 && idleMs !== null && idleMs > STALLED_RPC_MS)
		out.push(
			m.diagnose_codex_silence_stalled_rpc({ count: cx.pending_rpc_count, age: fmtAge(idleMs) })
		);
	// A shared-connection drop fails every in-flight request on that socket, so
	// inventory/lifecycle traffic stops while the stdio child still looks fine.
	const dropped = (cx.protocol_errors ?? []).filter(
		(e) => e.transport === 'shared' && e.message.includes('connection dropped')
	);
	if (dropped.length)
		out.push(
			m.diagnose_codex_silence_shared_dropped({
				count: dropped.length,
				age: fmtAge(generatedAtMs - dropped[dropped.length - 1].ts_ms)
			})
		);
	// Only meaningful once *some* traffic exists: frames on the stdio child but
	// none on the shared socket is the blind spot this tagging exists to expose.
	if (frames.length && !frames.some((f) => f.transport === 'shared'))
		out.push(m.diagnose_codex_silence_shared_no_frames());
	if (!cx.active_turn_id) out.push(m.diagnose_codex_silence_no_turn());
	if (cx.auth_state && !cx.auth_state.startsWith('gateway env present'))
		out.push(m.diagnose_codex_silence_auth({ state: cx.auth_state }));
	if (cx.registry_live_mismatch)
		out.push(m.diagnose_codex_silence_mismatch({ detail: cx.registry_live_mismatch }));
	if (!cx.live) out.push(m.diagnose_codex_silence_not_live());
	return out;
}

/** Same question for opencode. Its transports are HTTP and SSE, and the
 * asymmetry matters: a request/response call failing is loud (the caller sees
 * the status), while the event stream going down is completely silent — every
 * turn observation rides it. */
export function openCodeSilenceReasons(oc: OpenCodeDiagnose, generatedAtMs: number): string[] {
	const out: string[] = [];
	const frames = oc.rpc_tail ?? [];
	const working = oc.turn_status === 'working';
	if (!oc.sse_connected) out.push(m.diagnose_opencode_silence_sse_down());
	const eventAgeMs = oc.last_sse_event_ms === null || oc.last_sse_event_ms === undefined
		? null
		: generatedAtMs - oc.last_sse_event_ms;
	if (working && oc.sse_connected && eventAgeMs !== null && eventAgeMs > STALLED_RPC_MS)
		out.push(m.diagnose_opencode_silence_sse_stalled({ age: fmtAge(eventAgeMs) }));
	// Only meaningful once *some* traffic exists: HTTP frames with no SSE frame
	// is the blind spot the transport tagging exists to expose.
	if (frames.length && !frames.some((f) => f.transport === 'sse'))
		out.push(m.diagnose_opencode_silence_sse_no_frames());
	const rejected = (oc.protocol_errors ?? []).filter((e) => e.transport === 'http');
	if (rejected.length) {
		const last = rejected[rejected.length - 1];
		out.push(
			m.diagnose_opencode_silence_http_errors({
				count: rejected.length,
				age: fmtAge(generatedAtMs - last.ts_ms),
				message: last.message
			})
		);
	}
	const pending = oc.pending_permissions ?? [];
	if (pending.length)
		out.push(m.diagnose_opencode_silence_awaiting_permission({ count: pending.length }));
	if (!working) out.push(m.diagnose_opencode_silence_idle());
	if (oc.version_matches === false && oc.server_version)
		out.push(
			m.diagnose_opencode_silence_version({
				version: oc.server_version,
				pinned: oc.pinned_version
			})
		);
	if (!oc.live) out.push(m.diagnose_opencode_silence_not_live());
	return out;
}
