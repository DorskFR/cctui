<script lang="ts">
	import { Button, Text } from '@dorsk/tsumikit';
	import { goto } from '$app/navigation';
	import { m } from '$lib/paraglide/messages';
	import { settings } from '$lib/settings.svelte';
	import { tilesHref } from '$lib/tilesLink';
	import { toasts } from '$lib/toast.svelte';
	import type { SessionsPage } from './sessionsPage.svelte';

	let { sp }: { sp: SessionsPage } = $props();

	// The grid is capped, so a selection over it opens the first `maxTiles` and
	// says so rather than silently dropping the rest.
	function openInTiles() {
		const max = settings.state.tiles.maxTiles;
		const picked = [...sp.list.selected];
		if (picked.length > max) toasts.info(m.tiles_over_cap({ max }));
		sp.list.exitSelect();
		void goto(tilesHref(picked.slice(0, max)));
	}
</script>

<div class="bulkbar row">
	<Text class="count" size="sm" weight="semibold" tone="muted">{m.sessions_selected_count({ count: sp.list.selected.size })}</Text>
	<Button onclick={sp.list.selectAll}>{m.sessions_select_all()}</Button>
	<Text size="xs" tone="muted">{m.sessions_select_range_hint()}</Text>
	<div class="spacer"></div>
	<Button
		disabled={sp.list.selected.size === 0}
		onclick={openInTiles}
	>
		{m.tiles_open_count({ count: sp.list.selected.size || '' })}
	</Button>
	<Button
		variant="danger"
		loading={sp.archiving}
		disabled={sp.list.selected.size === 0 || sp.archiving}
		onclick={sp.archiveSelected}
	>
		{m.sessions_archive_count({ count: sp.list.selected.size || '' })}
	</Button>
</div>

<style>
	/* Sticky bulk-action bar shown while in select mode. */
	.bulkbar {
		position: sticky;
		top: calc(var(--header-h) + var(--safe-top) + var(--sp-2));
		z-index: 5;
		gap: var(--sp-2);
		align-items: center;
		margin-bottom: var(--sp-3);
		padding: var(--sp-2) var(--sp-3);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		background: var(--bg-elevated);
		box-shadow: var(--shadow-md);
	}
</style>
