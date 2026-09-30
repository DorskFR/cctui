<script lang="ts">
	import { Button, Popover, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { toasts } from '$lib/toast.svelte';
	import { errMessage } from '$lib/api';
	import { setSessionRoom, setSessionRoomByName } from '$lib/rooms';
	import RoomMenu from '$lib/components/molecules/RoomMenu.svelte';
	import type { SessionsPage } from './sessionsPage.svelte';

	let { sp }: { sp: SessionsPage } = $props();
	let making = $state(false);

	// One call per session: the room is a field on each, and `setSessionRoomByName`
	// creates-or-reuses so the whole selection lands in one room either way.
	async function moveToRoom(pick: { id: string } | { name: string }) {
		const picked = [...sp.list.selected];
		if (picked.length === 0) return;
		making = true;
		try {
			for (const id of picked) {
				if ('id' in pick) await setSessionRoom(id, pick.id);
				else await setSessionRoomByName(id, pick.name);
			}
			sp.list.exitSelect();
			sp.refreshTick++;
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			making = false;
		}
	}
</script>

<div class="bulkbar row">
	<Text class="count" size="sm" weight="semibold" tone="muted">{m.sessions_selected_count({ count: sp.list.selected.size })}</Text>
	<Button onclick={sp.list.selectAll}>{m.sessions_select_all()}</Button>
	<Text size="xs" tone="muted">{m.sessions_select_range_hint()}</Text>
	<div class="spacer"></div>
	<Popover label={m.rooms_move_count({ count: sp.list.selected.size || '' })} placement="bottom-end">
		{#snippet trigger()}
			<span>{m.rooms_move_count({ count: sp.list.selected.size || '' })}</span>
		{/snippet}
		<RoomMenu busy={making} onpick={moveToRoom} />
	</Popover>
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
