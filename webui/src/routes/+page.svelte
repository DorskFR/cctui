<script lang="ts">
	import { useUsers, useSessionStats, useTokenStats } from '$lib/queries';
	import { apiOrigin } from '$lib/config';
	import { toasts } from '$lib/toast.svelte';
	import TokenUsage from '$lib/components/molecules/TokenUsage.svelte';
	import { AutoGrid, Button, Card, Cluster, Heading, Stack, Text } from '@dorsk/tsumikit';
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

	const statCards = $derived([
		{ lbl: 'Live sessions', num: live },
		{ lbl: 'Need input', num: needs, warn: needs > 0 },
		{ lbl: 'Archived', num: archived },
		{ lbl: 'Active users', num: activeUsers },
		{ lbl: 'Revoked users', num: revokedUsers },
		{ lbl: 'Total sessions', num: total }
	]);

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

<Stack gap="var(--sp-6)">
		<Heading level={1}>Overview</Heading>

		<AutoGrid min="10rem" gap="var(--sp-3)">
			{#each statCards as c (c.lbl)}
				<Card>
					<Stack gap="var(--sp-1)" align="flex-start">
						<Text
							size="2xl"
							weight="bold"
							style="line-height: 1{c.warn ? '; color: var(--warn)' : ''}">{c.num}</Text
						>
						<Text size="sm" tone="muted">{c.lbl}</Text>
					</Stack>
				</Card>
			{/each}
		</AutoGrid>

		<Stack gap="var(--sp-3)">
			<Heading level={2} size="lg">Token usage</Heading>
			<AutoGrid min="10rem" gap="var(--sp-3)">
				{#each tokenCards as c (c.lbl)}
					<Card>
						<Stack gap="var(--sp-1)" align="flex-start">
							<TokenUsage usage={c.usage} showSum={false} size="lg" wrap />
							<Text size="sm" tone="muted">{c.lbl}</Text>
						</Stack>
					</Card>
				{/each}
			</AutoGrid>
		</Stack>

		<Card>
			<Stack>
				<Text weight="bold">Enroll a machine</Text>
				<Text as="p" tone="muted" size="sm">
					Install <Text variant="code">cctui-daemon</Text> on the target host (from GitHub Releases), then
					enroll it with a user token (create one on the Users page):
				</Text>
				<Cluster wrap={false} align="center">
					<!-- as="div": truncate needs a block element — text-overflow:ellipsis is
					     ignored on an inline <span>, so the long command would spill. -->
					<div class="cmd"><Text as="div" variant="code" truncate>{enrollCmd}</Text></div>
					<Button size="sm" onclick={copyEnroll}>Copy</Button>
				</Cluster>
				<Text as="p" tone="muted" size="sm">
					Then run it as a service: <Text variant="code">cctui-daemon service install</Text>
				</Text>
			</Stack>
		</Card>
</Stack>

<style>
	/* The enroll command box is structural chrome — a LOCAL element wrapping the
	   Text atom, so it styles itself with scoped CSS (no :global reach-in). It owns
	   the shrink (min-width:0 + flex:1) that lets the Text inside truncate. */
	.cmd {
		flex: 1;
		min-width: 0;
		padding: var(--sp-2) var(--sp-3);
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		font-size: var(--fs-xs);
	}
</style>
