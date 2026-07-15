<script lang="ts">
	import { useUsers, useSessionStats, useTokenStats } from '$lib/queries';
	import { apiOrigin } from '$lib/config';
	import { toasts } from '$lib/toast.svelte';
	import TokenUsage from '$lib/components/molecules/TokenUsage.svelte';
	import UsageAnalyticsSection from '$lib/components/organisms/overview/UsageAnalyticsSection.svelte';
	import { AutoGrid, Button, Card, Cluster, Heading, Stack, Text } from '@dorsk/tsumikit';
	import { asUsage } from './home.logic';
	import { m } from '$lib/paraglide/messages';

	const users = useUsers();
	// Aggregate counts from the server, not the capped session list — the list
	// tops out at 25 rows so counting it client-side undercounts (CCT).
	const stats = useSessionStats();
	// Token totals across rolling windows, same ↑in ↓out ⚡cache readout the
	// session list shows.
	const tokens = useTokenStats();

	const tokenCards = $derived([
		{ lbl: m.home_window_hour(), usage: asUsage($tokens.data?.hour) },
		{ lbl: m.home_window_today(), usage: asUsage($tokens.data?.today) },
		{ lbl: m.home_window_day(), usage: asUsage($tokens.data?.day) },
		{ lbl: m.home_window_week(), usage: asUsage($tokens.data?.week) },
		{ lbl: m.home_window_month(), usage: asUsage($tokens.data?.month) }
	]);

	const activeUsers = $derived(($users.data ?? []).filter((u) => !u.revoked_at).length);
	const revokedUsers = $derived(($users.data ?? []).filter((u) => u.revoked_at).length);
	const live = $derived($stats.data?.live ?? 0);
	const archived = $derived($stats.data?.archived ?? 0);
	const needs = $derived($stats.data?.needs_input ?? 0);
	const total = $derived($stats.data?.total ?? 0);

	const statCards = $derived([
		{ lbl: m.home_stat_live(), num: live },
		{ lbl: m.home_stat_needs_input(), num: needs, warn: needs > 0 },
		{ lbl: m.home_stat_archived(), num: archived },
		{ lbl: m.home_stat_active_users(), num: activeUsers },
		{ lbl: m.home_stat_revoked_users(), num: revokedUsers },
		{ lbl: m.home_stat_total_sessions(), num: total }
	]);

	const enrollCmd = $derived(
		`cctui-daemon enroll --server-url ${apiOrigin()} --token <user-token> --name "$(hostname)"`
	);

	async function copyEnroll() {
		try {
			await navigator.clipboard.writeText(enrollCmd);
			toasts.ok(m.common_copied());
		} catch {
			toasts.err(m.home_clipboard_unavailable());
		}
	}
</script>

<Stack gap="var(--sp-6)">
		<Heading level={1}>{m.home_overview_title()}</Heading>

		<AutoGrid min="10rem" gap="var(--sp-3)" maxCols={3}>
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
			<Heading level={2} size="lg">{m.home_token_usage()}</Heading>
			<AutoGrid min="10rem" gap="var(--sp-3)"  maxCols={3}>
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

		<UsageAnalyticsSection />

		<Card>
			<Stack>
				<Text weight="bold">{m.home_enroll_title()}</Text>
				<Text as="p" tone="muted" size="sm">
					{m.home_enroll_install_before()} <Text variant="code">cctui-daemon</Text>
					{m.home_enroll_install_after()}
				</Text>
				<Cluster wrap={false} align="center">
					<!-- as="div": truncate needs a block element — text-overflow:ellipsis is
					     ignored on an inline <span>, so the long command would spill. -->
					<div class="cmd"><Text as="div" variant="code" truncate>{enrollCmd}</Text></div>
					<Button onclick={copyEnroll}>{m.common_copy()}</Button>
				</Cluster>
				<Text as="p" tone="muted" size="sm">
					{m.home_enroll_run_as_service()} <Text variant="code">cctui-daemon service install</Text>
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
