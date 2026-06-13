<script lang="ts">
	import { useUsers, useSessionStats, useTokenStats } from '$lib/queries';
	import { apiOrigin } from '$lib/config';
	import { toasts } from '$lib/toast.svelte';
	import TokenUsage from '$lib/components/molecules/TokenUsage.svelte';
	import Button from '$lib/components/atoms/Button.svelte';
	import Heading from '$lib/components/atoms/Heading.svelte';
	import Text from '$lib/components/atoms/Text.svelte';
	import { asUsage } from './home.logic';

	const users = useUsers();
	// Aggregate counts from the server, not the capped session list — the list
	// tops out at 25 rows so counting it client-side undercounts (CCT).
	const stats = useSessionStats();
	// Token totals across rolling windows, same ↑in ↓out ⚡cache readout the
	// session list shows.
	const tokens = useTokenStats();

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

<div class="title">
	<Heading level={1}>Overview</Heading>
</div>

<div class="grid">
	<div class="card stat">
		<Text size="2xl" weight="bold" class="num">{live}</Text><Text size="sm" tone="muted">Live sessions</Text>
	</div>
	<div class="card stat">
		<Text size="2xl" weight="bold" class="num {needs > 0 ? 'warn' : ''}">{needs}</Text><Text size="sm" tone="muted">Need input</Text>
	</div>
	<div class="card stat">
		<Text size="2xl" weight="bold" class="num">{archived}</Text><Text size="sm" tone="muted">Archived</Text>
	</div>
	<div class="card stat">
		<Text size="2xl" weight="bold" class="num">{activeUsers}</Text><Text size="sm" tone="muted">Active users</Text>
	</div>
	<div class="card stat">
		<Text size="2xl" weight="bold" class="num">{revokedUsers}</Text><Text size="sm" tone="muted">Revoked users</Text>
	</div>
	<div class="card stat">
		<Text size="2xl" weight="bold" class="num">{total}</Text><Text size="sm" tone="muted">Total sessions</Text>
	</div>
</div>

<div class="section-title">
	<Heading level={2} size="lg">Token usage</Heading>
</div>
<div class="grid token-grid">
	{#each tokenCards as c (c.lbl)}
		<div class="card stat">
			<TokenUsage usage={c.usage} />
			<Text size="sm" tone="muted">{c.lbl}</Text>
		</div>
	{/each}
</div>

<div class="card install stack">
	<Text weight="bold">Enroll a machine</Text>
	<Text as="p" tone="muted" size="sm">
		Install <Text variant="code">cctui-daemon</Text> on the target host (from GitHub Releases), then
		enroll it with a user token (create one on the Users page):
	</Text>
	<div class="row">
		<Text variant="code" truncate class="cmd">{enrollCmd}</Text>
		<Button size="sm" onclick={copyEnroll}>Copy</Button>
	</div>
	<Text as="p" tone="muted" size="sm">
		Then run it as a service: <Text variant="code">cctui-daemon service install</Text>
	</Text>
</div>

<style>
	/* Typography from the Heading/Text atoms; only the page rhythm lives here. */
	.title {
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
	/* The stat figure tightens its line-box; the warn variant recolours it.
	   :global — these ride on the Text atom whose scoped class can't see ours. */
	.stat :global(.num) {
		line-height: 1;
	}
	.stat :global(.num.warn) {
		color: var(--warn);
	}
	/* The enroll command box is structural chrome around the Text atom. */
	.install .row :global(.cmd) {
		flex: 1;
		padding: var(--sp-2) var(--sp-3);
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		font-size: var(--fs-xs);
	}
</style>
