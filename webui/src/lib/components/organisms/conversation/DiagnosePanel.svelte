<script lang="ts">
	// Session diagnose panel: one call that renders everything the
	// daemon knows about this session — each fact dated + sourced, plus the
	// arbitration verdict — and the server-side gateway/account binding facts.
	// Read-only observability; the only action is an explicit refresh (the
	// call round-trips server → daemon → adapter, so no background polling).
	import { useSessionDiagnose } from '$lib/queries';
	import type { CodexDiagnose } from '@bindings/CodexDiagnose';
	import type { DiagnoseFact } from '@bindings/DiagnoseFact';
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { sessionEnd } from '$lib/sessionEnd';
	import { Button, Heading, Modal, Text, Timestamp } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		sessionId,
		session = null,
		onclose
	}: {
		sessionId: string;
		session?: SessionListItem | null;
		onclose: () => void;
	} = $props();

	const query = useSessionDiagnose(() => sessionId);
	const end = $derived(session ? sessionEnd(session) : null);

	function fmtAge(ms: number | null): string {
		if (ms === null) return m.diagnose_undated();
		if (ms < 1_000) return m.diagnose_age_ms({ ms });
		if (ms < 60_000) return m.diagnose_age_s({ s: Math.floor(ms / 1_000) });
		if (ms < 3_600_000) return m.diagnose_age_m({ min: Math.floor(ms / 60_000) });
		return m.diagnose_age_h({ h: Math.floor(ms / 3_600_000) });
	}

	// Compact one-line rendering of a fact value: strings as-is, objects as
	// `key: value` pairs with nulls dropped (the reason a field is absent is
	// carried by `missing_reason`, not by rendering nulls).
	function fmtValue(v: unknown): string {
		if (v === null || v === undefined) return '';
		if (typeof v === 'string') return v;
		if (typeof v !== 'object') return String(v);
		return Object.entries(v as Record<string, unknown>)
			.filter(([, val]) => val !== null && val !== undefined)
			.map(([k, val]) => `${k}: ${Array.isArray(val) ? val.join(', ') : String(val)}`)
			.join(' · ');
	}

	type Row = { name: string; fact: DiagnoseFact<unknown> };
	const rows = $derived.by((): Row[] => {
		const d = query.data?.daemon;
		if (!d) return [];
		return [
			{ name: m.diagnose_fact_effective_state(), fact: d.effective_state },
			{ name: m.diagnose_fact_last_hook_event(), fact: d.last_hook_event },
			{ name: m.diagnose_fact_attach(), fact: d.attach },
			{ name: m.diagnose_fact_pty_output(), fact: d.pty_output },
			{ name: m.diagnose_fact_claude_socket(), fact: d.claude_socket },
			{ name: m.diagnose_fact_transcript(), fact: d.transcript },
			{ name: m.diagnose_fact_pending_prompts(), fact: d.prompts },
			{ name: m.diagnose_fact_permission_mode(), fact: d.permission_mode },
			{ name: m.diagnose_fact_dispatch(), fact: d.dispatch },
			{ name: m.diagnose_fact_gateway(), fact: d.gateway }
		];
	});

	// For a codex session the claude-only facts come back as placeholder
	// `missing` rows sourced `codex`; hide those and let the Codex section carry
	// the real state.
	const visibleRows = $derived(
		rows.filter((r) => !(r.fact.value === null && r.fact.source === 'codex'))
	);

	function codexRows(cx: CodexDiagnose): { name: string; value: string }[] {
		const version = cx.codex_version
			? `${cx.codex_version}${cx.version_supported === false ? ` ${m.diagnose_below_min()}` : ''}`
			: m.diagnose_unknown();
		const turn = cx.active_turn_id ? `${cx.turn_status} · ${cx.active_turn_id}` : cx.turn_status;
		const pending =
			cx.pending_rpc_count > 0
				? `${cx.pending_rpc_count} (${cx.pending_rpc_methods.join(', ')})`
				: '0';
		const rollout = cx.rollout_path
			? `${cx.rollout_path}${cx.rollout_size_bytes !== null ? ` · ${cx.rollout_size_bytes} bytes` : ''}`
			: '—';
		const out: { name: string; value: string }[] = [
			{ name: m.diagnose_codex_version(), value: `${version} · pinned ${cx.pinned_version} · min ${cx.min_version}` },
			{
				name: m.diagnose_codex_app_server(),
				value: `${cx.transport}${cx.app_server_pid !== null ? ` · pid ${cx.app_server_pid}` : ''} · live ${cx.live} · registered ${cx.registered}`
			},
			{ name: m.diagnose_codex_thread(), value: cx.thread_id ?? '—' },
			{ name: m.diagnose_codex_turn(), value: turn },
			{ name: m.diagnose_codex_pending_rpcs(), value: pending },
			{ name: m.diagnose_codex_rollout(), value: rollout }
		];
		if (cx.auth_state) out.push({ name: m.diagnose_codex_auth(), value: cx.auth_state });
		if (cx.registry_live_mismatch)
			out.push({ name: m.diagnose_codex_registry_mismatch(), value: cx.registry_live_mismatch });
		return out;
	}

	const STALLED_RPC_MS = 60_000;

	// Each entry is an independent reason the session can look silent; they are
	// derived from the codex facts alone, no extra sensing.
	function silenceReasons(cx: CodexDiagnose, generatedAtMs: number): string[] {
		const out: string[] = [];
		const frames = cx.rpc_tail ?? [];
		const lastFrameMs = frames.length ? frames[frames.length - 1].ts_ms : null;
		const idleMs = lastFrameMs === null ? null : generatedAtMs - lastFrameMs;
		if (cx.pending_rpc_count > 0 && idleMs !== null && idleMs > STALLED_RPC_MS)
			out.push(
				m.diagnose_codex_silence_stalled_rpc({ count: cx.pending_rpc_count, age: fmtAge(idleMs) })
			);
		if (!cx.active_turn_id) out.push(m.diagnose_codex_silence_no_turn());
		if (cx.auth_state && !cx.auth_state.startsWith('gateway env present'))
			out.push(m.diagnose_codex_silence_auth({ state: cx.auth_state }));
		if (cx.registry_live_mismatch)
			out.push(m.diagnose_codex_silence_mismatch({ detail: cx.registry_live_mismatch }));
		if (!cx.live) out.push(m.diagnose_codex_silence_not_live());
		return out;
	}

	function stderrText(cx: CodexDiagnose, generatedAtMs: number): string {
		return (cx.stderr_tail ?? []).map((l) => `${fmtAge(generatedAtMs - l.ts_ms)}  ${l.line}`).join('\n');
	}

	function rpcText(cx: CodexDiagnose, generatedAtMs: number): string {
		return (cx.rpc_tail ?? [])
			.map(
				(f) =>
					`${fmtAge(generatedAtMs - f.ts_ms)}  ${f.direction === 'out' ? '→' : '←'} ${f.label}  ${f.json}`
			)
			.join('\n');
	}
</script>

<Modal title={m.diagnose_title()} size="lg" onclose={onclose}>
	{#snippet body()}
		<div class="diag-body">
			<Text size="xs" tone="muted">{sessionId}</Text>

			{#if session}
				<Heading level={4}>{m.diagnose_end_of_life()}</Heading>
				{#if end}
					<div class="facts" role="table" aria-label={m.diagnose_end_of_life()}>
						<div class="fact codex-fact" role="row">
							<span class="name">{m.diagnose_end_reason()}</span>
							<span class="val">{end.label} ({end.reason})</span>
						</div>
						<div class="fact codex-fact" role="row">
							<span class="name">{m.diagnose_end_at()}</span>
							<span class="val">{#if end.endedAt}<Timestamp value={end.endedAt} tone="inherit" />{:else}—{/if}</span>
						</div>
						{#if end.detail}
							<div class="fact codex-fact" role="row">
								<span class="name">{m.diagnose_end_detail()}</span>
								<pre class="val end-detail">{end.detail}</pre>
							</div>
						{/if}
					</div>
				{:else}
					<Text size="sm" tone="muted">{m.diagnose_end_none()}</Text>
				{/if}
			{/if}

			{#if query.isLoading}
				<Text size="sm" tone="muted">{m.diagnose_asking()}</Text>
			{:else if query.error}
				<Text size="sm" tone="danger">
					{query.error instanceof Error ? query.error.message : m.diagnose_failed()}
				</Text>
			{:else if query.data}
				{@const resp = query.data}
				<div class="server-facts">
					<span class="src">{m.diagnose_src_server()}</span>
					<span>
						status: {resp.server.status ?? '?'} · adapter: {resp.server.adapter_id ?? '?'} ·
						account: {resp.server.account_bound ? resp.server.accounts.join(', ') : m.diagnose_not_bound()}
						{#if resp.server.machine_last_seen_ms != null}
							· {m.diagnose_daemon_heartbeat({ age: fmtAge(Date.now() - (resp.server.machine_last_seen_ms ?? 0)) })}
						{/if}
					</span>
				</div>

				{#if resp.daemon_error}
					<div class="daemon-error">
						<Text size="sm" tone="danger">{m.diagnose_daemon_unavailable({ error: resp.daemon_error })}</Text>
					</div>
				{/if}

				{#if resp.daemon}
					<Text size="xs" tone="muted">
						{m.diagnose_report_from({ adapter: resp.daemon.adapter, worker: resp.daemon.short ?? '?' })}
					</Text>
					<div class="facts" role="table" aria-label={m.diagnose_facts_aria()}>
						{#each visibleRows as row (row.name)}
							<div class="fact" role="row">
								<span class="name">{row.name}</span>
								<span class="meta">
									<span class="src">{row.fact.source}</span>
									<span class="age">{fmtAge(row.fact.age_ms ?? null)}</span>
								</span>
								{#if row.fact.value !== null}
									<span class="val">{fmtValue(row.fact.value)}</span>
								{:else}
									<span class="val missing">— {row.fact.missing_reason ?? m.diagnose_missing()}</span>
								{/if}
							</div>
						{/each}
					</div>

					{#if resp.daemon.codex}
						{@const cx = resp.daemon.codex}
						<Heading level={4}>Codex</Heading>
						<div class="facts" role="table" aria-label={m.diagnose_codex_facts_aria()}>
							{#each codexRows(cx) as row (row.name)}
								<div class="fact codex-fact" role="row">
									<span class="name">{row.name}</span>
									<span class="val">{row.value}</span>
								</div>
							{/each}
						</div>

						{@const reasons = silenceReasons(cx, resp.daemon.generated_at_ms)}
						<Heading level={4}>{m.diagnose_codex_silence()}</Heading>
						{#if reasons.length}
							<ul class="silence">
								{#each reasons as reason (reason)}
									<li>{reason}</li>
								{/each}
							</ul>
						{:else}
							<Text size="sm" tone="muted">{m.diagnose_codex_silence_none()}</Text>
						{/if}

						{#if cx.protocol_errors?.length}
							<Heading level={4}>{m.diagnose_codex_protocol_errors()}</Heading>
							<ul class="silence">
								{#each cx.protocol_errors ?? [] as err (err.ts_ms + err.message)}
									<li>
										<span class="age">{fmtAge(resp.daemon.generated_at_ms - err.ts_ms)}</span>
										{err.message}
									</li>
								{/each}
							</ul>
						{/if}

						<Heading level={4}>{m.diagnose_codex_stderr_tail({ count: cx.stderr_tail?.length ?? 0 })}</Heading>
						{#if cx.stderr_tail?.length}
							<pre class="tail">{stderrText(cx, resp.daemon.generated_at_ms)}</pre>
						{:else}
							<Text size="sm" tone="muted">{m.diagnose_codex_stderr_empty()}</Text>
						{/if}

						<Heading level={4}>{m.diagnose_codex_rpc_tail({ count: cx.rpc_tail?.length ?? 0 })}</Heading>
						{#if cx.rpc_tail?.length}
							<pre class="tail">{rpcText(cx, resp.daemon.generated_at_ms)}</pre>
						{:else}
							<Text size="sm" tone="muted">{m.diagnose_codex_rpc_empty()}</Text>
						{/if}
					{/if}
				{/if}
			{/if}
		</div>
	{/snippet}
	{#snippet footer()}
		<Button size="sm" variant="ghost" onclick={() => query.refetch()} loading={query.isFetching}>
			{m.diagnose_refresh()}
		</Button>
		<Button size="sm" variant="ghost" onclick={onclose}>{m.common_close()}</Button>
	{/snippet}
</Modal>

<style>
	.tail {
		margin: 0;
		white-space: pre-wrap;
		word-break: break-word;
		font-family: var(--font-mono, monospace);
		font-size: var(--fs-xs);
		max-height: 14rem;
		overflow: auto;
	}
	.silence {
		margin: 0;
		padding-left: var(--sp-4);
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.end-detail {
		margin: 0;
		white-space: pre-wrap;
		word-break: break-word;
		font-family: var(--font-mono, monospace);
		font-size: var(--fs-xs);
		max-height: 12rem;
		overflow: auto;
	}
	.diag-body {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		font-size: var(--fs-sm);
	}
	.server-facts {
		display: flex;
		gap: var(--sp-2);
		align-items: baseline;
		padding: var(--sp-2);
		background: var(--bg-elevated-2);
		border-radius: var(--r-md);
	}
	.facts {
		display: flex;
		flex-direction: column;
	}
	.fact {
		display: grid;
		grid-template-columns: 9rem 11rem 1fr;
		gap: var(--sp-2);
		align-items: baseline;
		padding: var(--sp-1) 0;
		border-bottom: 1px solid var(--border);
	}
	.fact:last-child {
		border-bottom: none;
	}
	.name {
		font-weight: 600;
		white-space: nowrap;
	}
	.meta {
		display: flex;
		gap: var(--sp-1);
		align-items: baseline;
		white-space: nowrap;
	}
	.src {
		font-size: var(--fs-xs);
		padding: 0 var(--sp-1);
		border: 1px solid var(--border);
		border-radius: var(--r-sm, 4px);
		color: var(--text-muted);
	}
	.age {
		font-size: var(--fs-xs);
		color: var(--text-muted);
	}
	.val {
		overflow-wrap: anywhere;
	}
	.val.missing {
		color: var(--text-muted);
		font-style: italic;
	}
	.daemon-error {
		padding: var(--sp-2);
		border: 1px solid var(--danger);
		border-radius: var(--r-md);
	}
	@media (max-width: 639px) {
		.fact {
			grid-template-columns: 1fr;
			gap: 2px;
		}
	}
</style>
