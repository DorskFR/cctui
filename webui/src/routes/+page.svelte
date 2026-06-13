<script lang="ts">
	import { useUsers, useSessionStats, useTokenStats } from '$lib/queries';
	import { apiOrigin } from '$lib/config';
	import { toasts } from '$lib/toast.svelte';
	import TokenUsage from '$lib/components/molecules/TokenUsage.svelte';
	import Button from '$lib/components/atoms/Button.svelte';
	import type { WindowTokenUsage } from '@bindings/WindowTokenUsage';
	import type { TokenUsage as TokenUsageT } from '@bindings/TokenUsage';

	const users = useUsers();
	// Aggregate counts from the server, not the capped session list — the list
	// tops out at 25 rows so counting it client-side undercounts (CCT).
	const stats = useSessionStats();
	// Token totals across rolling windows, same ↑in ↓out ⚡cache readout the
	// session list shows.
	const tokens = useTokenStats();

	// Adapt a window's {input, output, cache_read} to the shape TokenUsage.svelte
	// renders (the session-card readout). cost_usd / cache_creation are unused here.
	const asUsage = (w: WindowTokenUsage | undefined): TokenUsageT => ({
		tokens_in: w?.input ?? 0,
		tokens_out: w?.output ?? 0,
		cost_usd: 0,
		cache_read_tokens: w?.cache_read ?? 0,
		cache_creation_tokens: 0
	});

	const tokenCards = $derived([
		{ lbl: 'Last hour', usage: asUsage($tokens.data?.hour) },
		{ lbl: 'Today', usage: asUsage($tokens.data?.today) },
		{ lbl: 'Last 24h', usage: asUsage($tokens.data?.day) },
		{ lbl: 'Last 7d', usage: asUsage($tokens.data?.week) },
		{ lbl: 'Last 30d', usage: asUsage($tokens.data?.month) }
	]);

	const activeUsers = $derived(($users.data ?? []).filter((u) => !u.revoked_at).length);
	const revokedUsers = $derived(($users.data ?? []).filter((u) => u.revoked_at).length);
	const live = $derived($stats.data?.live ?? 0);
	const archived = $derived($stats.data?.archived ?? 0);
	const needs = $derived($stats.data?.needs_input ?? 0);
	const total = $derived($stats.data?.total ?? 0);

	const enrollCmd = $derived(
		`cctui-daemon enroll --server-url ${apiOrigin()} --token <user-token> --name "$(hostname)"`
	);

	async function copyEnroll() {
		try {
			await navigator.clipboard.writeText(enrollCmd);
			toasts.ok('Copied');
		} catch {
			toasts.err('Clipboard unavailable');
		}
	}
</script>

<h1 class="page-title">Overview</h1>

<div class="grid">
	<div class="card stat">
		<span class="num">{live}</span><span class="lbl">Live sessions</span>
	</div>
	<div class="card stat">
		<span class="num" class:warn={needs > 0}>{needs}</span><span class="lbl">Need input</span>
	</div>
	<div class="card stat">
		<span class="num">{archived}</span><span class="lbl">Archived</span>
	</div>
	<div class="card stat">
		<span class="num">{activeUsers}</span><span class="lbl">Active users</span>
	</div>
	<div class="card stat">
		<span class="num">{revokedUsers}</span><span class="lbl">Revoked users</span>
	</div>
	<div class="card stat">
		<span class="num">{total}</span><span class="lbl">Total sessions</span>
	</div>
</div>

<h2 class="section-title">Token usage</h2>
<div class="grid token-grid">
	{#each tokenCards as c (c.lbl)}
		<div class="card stat">
			<TokenUsage usage={c.usage} />
			<span class="lbl">{c.lbl}</span>
		</div>
	{/each}
</div>

<div class="card install stack">
	<strong>Enroll a machine</strong>
	<p class="muted">
		Install <code class="mono">cctui-daemon</code> on the target host (from GitHub Releases), then
		enroll it with a user token (create one on the Users page):
	</p>
	<div class="row">
		<code class="cmd mono truncate">{enrollCmd}</code>
		<Button size="sm" onclick={copyEnroll}>Copy</Button>
	</div>
	<p class="muted">
		Then run it as a service: <code class="mono">cctui-daemon service install</code>
	</p>
</div>

<style>
	.page-title {
		font-size: var(--fs-2xl);
		margin-bottom: var(--sp-4);
	}
	.grid {
		display: grid;
		grid-template-columns: repeat(2, 1fr);
		gap: var(--sp-3);
		margin-bottom: var(--sp-4);
	}
	@media (min-width: 640px) {
		.grid {
			grid-template-columns: repeat(3, 1fr);
		}
	}
	.section-title {
		font-size: var(--fs-lg);
		margin-bottom: var(--sp-3);
	}
	.token-grid {
		margin-bottom: var(--sp-4);
	}
	.stat {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		align-items: flex-start;
	}
	.num {
		font-size: var(--fs-2xl);
		font-weight: var(--fw-bold);
		line-height: 1;
	}
	.num.warn {
		color: var(--warn);
	}
	.lbl {
		font-size: var(--fs-sm);
		color: var(--text-muted);
	}
	.install .cmd {
		flex: 1;
		padding: var(--sp-2) var(--sp-3);
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		font-size: var(--fs-xs);
	}
</style>
