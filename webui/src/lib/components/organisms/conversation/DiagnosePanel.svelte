<script lang="ts">
	// Session diagnose panel: one call that renders everything the
	// daemon knows about this session — each fact dated + sourced, plus the
	// arbitration verdict — and the server-side gateway/account binding facts.
	// Read-only observability; the only action is an explicit refresh (the
	// call round-trips server → daemon → adapter, so no background polling).
	import { useSessionDiagnose } from '$lib/queries';
	import type { CodexDiagnose } from '@bindings/CodexDiagnose';
	import type { DiagnoseFact } from '@bindings/DiagnoseFact';
	import { Button, Heading, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		sessionId,
		onclose
	}: {
		sessionId: string;
		onclose: () => void;
	} = $props();

	const query = useSessionDiagnose(() => sessionId);

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
		if (cx.last_protocol_error)
			out.push({ name: m.diagnose_codex_last_protocol_error(), value: cx.last_protocol_error });
		if (cx.registry_live_mismatch)
			out.push({ name: m.diagnose_codex_registry_mismatch(), value: cx.registry_live_mismatch });
		return out;
	}
</script>

<div
	class="diag-scrim"
	role="button"
	tabindex="-1"
	aria-label={m.diagnose_close_aria()}
	onclick={onclose}
	onkeydown={(e) => e.key === 'Escape' && onclose()}
></div>
<div class="diag-modal" role="dialog" aria-modal="true" aria-label={m.diagnose_title()}>
	<div class="diag-head">
		<Heading level={3}>{m.diagnose_title()}</Heading>
		<div class="diag-actions">
			<Button size="sm" variant="ghost" onclick={() => query.refetch()} loading={query.isFetching}>
				{m.diagnose_refresh()}
			</Button>
			<Button size="sm" variant="ghost" onclick={onclose}>{m.common_close()}</Button>
		</div>
	</div>
	<Text size="xs" tone="muted">{sessionId}</Text>

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
				{#if resp.server.machine_last_seen_ms !== null}
					· {m.diagnose_daemon_heartbeat({ age: fmtAge(Date.now() - resp.server.machine_last_seen_ms) })}
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
							<span class="age">{fmtAge(row.fact.age_ms)}</span>
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
			{/if}
		{/if}
	{/if}
</div>

<style>
	.diag-scrim {
		position: fixed;
		inset: 0;
		z-index: 70;
		background: rgba(0, 0, 0, 0.45);
		border: none;
	}
	.diag-modal {
		position: fixed;
		z-index: 71;
		top: 50%;
		left: 50%;
		transform: translate(-50%, -50%);
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		width: min(42rem, calc(100vw - 2rem));
		max-height: calc(100vh - 4rem);
		overflow-y: auto;
		padding: var(--sp-4);
		background: var(--bg-elevated);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-lg, var(--r-md));
		box-shadow: var(--shadow-lg, 0 8px 24px rgba(0, 0, 0, 0.5));
		font-size: var(--fs-sm);
	}
	.diag-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
	}
	.diag-actions {
		display: flex;
		gap: var(--sp-1);
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
