<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { Badge, Button, EmptyState, Input, Text } from '@dorsk/tsumikit';
	import { errMessage } from '$lib/api';
	import { m } from '$lib/paraglide/messages';
	import { toasts } from '$lib/toast.svelte';
	import { ws } from '$lib/ws.svelte';
	import PageHead from '$lib/components/molecules/PageHead.svelte';
	import RoomPanel from '$lib/components/organisms/RoomPanel.svelte';
	import { addRoomMember, createRoom, joinableRooms, listRooms, type Room } from '$lib/rooms';

	let rooms = $state<Room[]>([]);
	let picked = $state<string | null>(null);
	let newName = $state('');
	let busy = $state(false);
	let panel = $state<RoomPanel | null>(null);

	// `?room=` lets the card badge deep-link straight into a room; `?add=` is the
	// drawer's "Add to room…", which lands here to choose which one.
	const wanted = $derived(page.url.searchParams.get('room'));
	const adding = $derived(page.url.searchParams.get('add'));
	const current = $derived(rooms.find((r) => r.id === (picked ?? wanted)) ?? rooms[0] ?? null);
	const candidates = $derived(adding ? joinableRooms(rooms, adding) : []);

	async function addTo(roomId: string) {
		if (!adding) return;
		busy = true;
		try {
			await addRoomMember(roomId, adding);
			picked = roomId;
			await load();
			await goto(`/rooms?room=${roomId}`, { replaceState: true });
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			busy = false;
		}
	}

	async function load() {
		try {
			rooms = await listRooms();
		} catch (e) {
			toasts.error(errMessage(e));
		}
	}

	$effect(() => {
		void load();
	});

	// A post or a membership change anywhere reaches this page as a server event;
	// the open panel catches up rather than polling.
	$effect(() => {
		if (ws.roomTick === 0) return;
		void panel?.refresh();
		void load();
	});

	async function create() {
		const name = newName.trim();
		if (!name) return;
		busy = true;
		try {
			const room = await createRoom(name);
			newName = '';
			picked = room.id;
			await load();
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			busy = false;
		}
	}
</script>

<PageHead title={m.rooms_title()} />

<div class="layout">
	<aside class="list">
		<form
			class="create"
			onsubmit={(e) => {
				e.preventDefault();
				void create();
			}}
		>
			<Input
				bind:value={newName}
				size="sm"
				grow
				disabled={busy}
				placeholder={m.rooms_name_placeholder()}
				aria-label={m.rooms_name_label()}
				maxlength={60}
			/>
			<Button size="sm" disabled={busy || !newName.trim()} onclick={create}>
				{m.rooms_create()}
			</Button>
		</form>

		{#if adding}
			<div class="adding">
				<Text size="xs" tone="muted">{m.rooms_add_to()}</Text>
				{#each candidates as room (room.id)}
					<Button
						size="sm"
						block
						style="justify-content:flex-start;text-align:left;min-width:0"
						disabled={busy}
						onclick={() => addTo(room.id)}
					>
						{room.name}
					</Button>
				{/each}
				{#if candidates.length === 0}
					<Text size="xs" tone="muted">{m.rooms_add_to_new()}</Text>
				{/if}
			</div>
		{/if}

		{#if rooms.length === 0}
			<EmptyState title={m.rooms_none()} />
		{:else}
			<ul class="rows">
				{#each rooms as room (room.id)}
					<li>
						<Button
							variant={current?.id === room.id ? 'primary' : 'ghost'}
							size="sm"
							block
							style="justify-content:flex-start;text-align:left;min-width:0"
							onclick={() => (picked = room.id)}
						>
							<span class="rowname">{room.name}</span>
							{#if room.archived}
								<Badge size="xs" uppercase>{m.rooms_archived()}</Badge>
							{/if}
							<Text size="xs" tone="muted">
								{m.rooms_members_count({ count: room.members.length })}
							</Text>
						</Button>
					</li>
				{/each}
			</ul>
		{/if}
	</aside>

	<div class="detail">
		{#if current}
			{#key current.id}
				<RoomPanel bind:this={panel} room={current} onchanged={load} />
			{/key}
		{:else}
			<EmptyState title={m.rooms_none()} />
		{/if}
	</div>
</div>

<style>
	.layout {
		display: grid;
		grid-template-columns: minmax(12rem, 18rem) minmax(0, 1fr);
		gap: var(--sp-4);
		align-items: start;
		min-height: 0;
	}
	.list {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		min-width: 0;
	}
	.create {
		display: flex;
		gap: var(--sp-2);
		align-items: center;
	}
	.adding {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		padding: var(--sp-2);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
	}
	.rows {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.rowname {
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
	.detail {
		min-width: 0;
	}
	/* One column below the fold: the room list becomes a header strip. */
	@media (max-width: 52rem) {
		.layout {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
