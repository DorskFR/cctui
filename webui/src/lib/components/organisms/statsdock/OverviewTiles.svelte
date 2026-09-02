<script lang="ts">
	import { useSessionStats, useUsers } from '$lib/queries';
	import { Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	// The Overview's headline counts as small tiles. Same sources as the
	// Overview page: server-side aggregates, not the capped session list.
	const stats = useSessionStats();
	const users = useUsers();
	const activeUsers = $derived((users.data ?? []).filter((u) => !u.revoked_at).length);
	const revokedUsers = $derived((users.data ?? []).filter((u) => u.revoked_at).length);
	const needs = $derived(stats.data?.needs_input ?? 0);
	const tiles = $derived([
		{ lbl: m.home_stat_live(), num: stats.data?.live ?? 0, warn: false },
		{ lbl: m.home_stat_needs_input(), num: needs, warn: needs > 0 },
		{ lbl: m.home_stat_archived(), num: stats.data?.archived ?? 0, warn: false },
		{ lbl: m.home_stat_active_users(), num: activeUsers, warn: false },
		{ lbl: m.home_stat_revoked_users(), num: revokedUsers, warn: false },
		{ lbl: m.home_stat_total_sessions(), num: stats.data?.total ?? 0, warn: false }
	]);
</script>

<div class="tiles">
	{#each tiles as t (t.lbl)}
		<div class="tile">
			<Text size="lg" weight="bold" numeric style={`line-height: 1${t.warn ? '; color: var(--warn)' : ''}`}>{t.num}</Text>
			<Text size="xs" tone="muted">{t.lbl}</Text>
		</div>
	{/each}
</div>

<style>
	.tiles {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: var(--sp-2);
	}
	.tile {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		padding: var(--sp-2);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		background: var(--bg-elevated-2);
		min-width: 0;
	}
</style>
