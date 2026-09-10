import type { CodexDiagnose } from '@bindings/CodexDiagnose';
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
