<script lang="ts">
	// Session diagnose panel (CCT-547): one call that renders everything the
	// daemon knows about this session — each fact dated + sourced, plus the
	// arbitration verdict — and the server-side gateway/account binding facts.
	// Read-only observability; the only action is an explicit refresh (the
	// call round-trips server → daemon → adapter, so no background polling).
	import { useSessionDiagnose } from '$lib/queries';
	import type { CodexDiagnose } from '@bindings/CodexDiagnose';
	import type { DiagnoseFact } from '@bindings/DiagnoseFact';
	import { Button, Heading, Text } from '@dorsk/tsumikit';

	let {
		sessionId,
		onclose
	}: {
		sessionId: string;
		onclose: () => void;
	} = $props();

	const query = useSessionDiagnose(() => sessionId);

	function fmtAge(ms: number | null): string {
		if (ms === null) return 'undated';
		if (ms < 1_000) return `${ms}ms ago`;
		if (ms < 60_000) return `${Math.floor(ms / 1_000)}s ago`;
		if (ms < 3_600_000) return `${Math.floor(ms / 60_000)}m ago`;
		return `${Math.floor(ms / 3_600_000)}h ago`;
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
		const d = $query.data?.daemon;
		if (!d) return [];
		return [
			{ name: 'effective state', fact: d.effective_state },
			{ name: 'last hook event', fact: d.last_hook_event },
			{ name: 'attach', fact: d.attach },
			{ name: 'PTY output', fact: d.pty_output },
			{ name: 'claude socket', fact: d.claude_socket },
			{ name: 'transcript', fact: d.transcript },
			{ name: 'pending prompts', fact: d.prompts },
			{ name: 'permission mode', fact: d.permission_mode },
			{ name: 'dispatch', fact: d.dispatch },
			{ name: 'gateway', fact: d.gateway }
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
			? `${cx.codex_version}${cx.version_supported === false ? ' (below min!)' : ''}`
			: 'unknown';
		const turn = cx.active_turn_id ? `${cx.turn_status} · ${cx.active_turn_id}` : cx.turn_status;
		const pending =
			cx.pending_rpc_count > 0
				? `${cx.pending_rpc_count} (${cx.pending_rpc_methods.join(', ')})`
				: '0';
		const rollout = cx.rollout_path
			? `${cx.rollout_path}${cx.rollout_size_bytes !== null ? ` · ${cx.rollout_size_bytes} bytes` : ''}`
			: '—';
		const out: { name: string; value: string }[] = [
			{ name: 'version', value: `${version} · pinned ${cx.pinned_version} · min ${cx.min_version}` },
			{
				name: 'app-server',
				value: `${cx.transport}${cx.app_server_pid !== null ? ` · pid ${cx.app_server_pid}` : ''} · live ${cx.live} · registered ${cx.registered}`
			},
			{ name: 'thread', value: cx.thread_id ?? '—' },
			{ name: 'turn', value: turn },
			{ name: 'pending RPCs', value: pending },
			{ name: 'rollout', value: rollout }
		];
		if (cx.auth_state) out.push({ name: 'auth', value: cx.auth_state });
		if (cx.last_protocol_error)
			out.push({ name: 'last protocol error', value: cx.last_protocol_error });
		if (cx.registry_live_mismatch)
			out.push({ name: 'registry mismatch', value: cx.registry_live_mismatch });
		return out;
	}
</script>

<div
	class="diag-scrim"
	role="button"
	tabindex="-1"
	aria-label="Close diagnose panel"
	onclick={onclose}
	onkeydown={(e) => e.key === 'Escape' && onclose()}
></div>
<div class="diag-modal" role="dialog" aria-modal="true" aria-label="Session diagnose">
	<div class="diag-head">
		<Heading level={3}>Session diagnose</Heading>
		<div class="diag-actions">
			<Button size="sm" variant="ghost" onclick={() => $query.refetch()} loading={$query.isFetching}>
				Refresh
			</Button>
			<Button size="sm" variant="ghost" onclick={onclose}>Close</Button>
		</div>
	</div>
	<Text size="xs" tone="muted">{sessionId}</Text>

	{#if $query.isLoading}
		<Text size="sm" tone="muted">Asking the daemon…</Text>
	{:else if $query.error}
		<Text size="sm" tone="danger">
			{$query.error instanceof Error ? $query.error.message : 'diagnose failed'}
		</Text>
	{:else if $query.data}
		{@const resp = $query.data}
		<div class="server-facts">
			<span class="src">server</span>
			<span>
				status: {resp.server.status ?? '?'} · adapter: {resp.server.adapter_id ?? '?'} ·
				account: {resp.server.account_bound ? resp.server.accounts.join(', ') : 'not bound'}
				{#if resp.server.machine_last_seen_ms !== null}
					· daemon heartbeat {fmtAge(Date.now() - resp.server.machine_last_seen_ms)}
				{/if}
			</span>
		</div>

		{#if resp.daemon_error}
			<div class="daemon-error">
				<Text size="sm" tone="danger">daemon unavailable — {resp.daemon_error}</Text>
			</div>
		{/if}

		{#if resp.daemon}
			<Text size="xs" tone="muted">
				report from daemon · adapter {resp.daemon.adapter} · worker {resp.daemon.short ?? '?'}
			</Text>
			<div class="facts" role="table" aria-label="Diagnose facts">
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
							<span class="val missing">— {row.fact.missing_reason ?? 'missing'}</span>
						{/if}
					</div>
				{/each}
			</div>

			{#if resp.daemon.codex}
				{@const cx = resp.daemon.codex}
				<Heading level={4}>Codex</Heading>
				<div class="facts" role="table" aria-label="Codex diagnose facts">
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
