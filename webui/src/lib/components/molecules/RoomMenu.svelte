<script lang="ts">
	import { Badge, Button, EmptyState, IconButton, Input, Text } from '@dorsk/tsumikit';
	import { errMessage } from '$lib/api';
	import { m } from '$lib/paraglide/messages';
	import { toasts } from '$lib/toast.svelte';
	import {
		canCreate,
		deleteRoom,
		listRooms,
		matchByName,
		pickable,
		renameRoom,
		setRoomArchived,
		type Room
	} from '$lib/rooms';

	// The one room picker: which room these sessions go in, plus the minimal
	// housekeeping (rename / archive / delete) that would otherwise need a page.
	let {
		current = null,
		busy = false,
		onpick,
		onclear
	}: {
		/** The room the subject is already in, so it reads as selected. */
		current?: string | null;
		busy?: boolean;
		/** An existing room id, or a name to create-or-reuse. */
		onpick: (pick: { id: string } | { name: string }) => void;
		/** Omit to hide the clear row (a multi-selection has nothing to clear). */
		onclear?: () => void;
	} = $props();

	let rooms = $state<Room[]>([]);
	let query = $state('');
	let working = $state(false);
	let renaming = $state<string | null>(null);
	let renameTo = $state('');

	const ROW = 'justify-content:flex-start;text-align:left;min-width:0;font-size:var(--fs-sm)';
	const live = $derived(pickable(rooms));
	const filtered = $derived(
		query.trim()
			? live.filter((r) => r.name.toLowerCase().includes(query.trim().toLowerCase()))
			: live
	);
	const offerCreate = $derived(canCreate(rooms, query));

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

	function submit() {
		const name = query.trim();
		if (!name) return;
		const hit = matchByName(rooms, name);
		onpick(hit ? { id: hit.id } : { name });
		query = '';
	}

	async function house(action: () => Promise<unknown>) {
		working = true;
		try {
			await action();
			await load();
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			working = false;
		}
	}
</script>

<div class="menu">
	<form
		class="find"
		onsubmit={(e) => {
			e.preventDefault();
			submit();
		}}
	>
		<Input
			bind:value={query}
			size="sm"
			grow
			disabled={busy || working}
			placeholder={m.rooms_name_placeholder()}
			aria-label={m.rooms_name_label()}
			maxlength={60}
		/>
	</form>

	{#if offerCreate}
		<Button
			variant="ghost"
			size="sm"
			block
			style={ROW}
			disabled={busy || working}
			onclick={submit}
		>
			<span class="glyph" aria-hidden="true">+</span>
			<span class="rowname">{query.trim()}</span>
		</Button>
	{/if}

	{#each filtered as room (room.id)}
		<div class="row">
			{#if renaming === room.id}
				<Input
					bind:value={renameTo}
					size="sm"
					grow
					disabled={working}
					aria-label={m.rooms_name_label()}
					maxlength={60}
					onkeydown={(e: KeyboardEvent) => {
						if (e.key === 'Enter' && renameTo.trim()) {
							void house(() => renameRoom(room.id, renameTo.trim()));
							renaming = null;
						}
						if (e.key === 'Escape') renaming = null;
					}}
				/>
			{:else}
				<Button
					variant={current === room.id ? 'primary' : 'ghost'}
					size="sm"
					block
					style={ROW}
					disabled={busy || working}
					onclick={() => onpick({ id: room.id })}
				>
					<span class="glyph" aria-hidden="true">{current === room.id ? '✓' : '◎'}</span>
					<span class="rowname">{room.name}</span>
					<Badge size="xs">{m.rooms_members_count({ count: room.members.length })}</Badge>
				</Button>
				<IconButton
					icon="edit"
					label={m.rooms_rename()}
					inline
					size={13}
					disabled={working}
					onclick={() => {
						renaming = room.id;
						renameTo = room.name;
					}}
				/>
				<IconButton
					icon="archive"
					label={m.rooms_archive()}
					inline
					size={13}
					disabled={working}
					onclick={() => house(() => setRoomArchived(room.id, true))}
				/>
				<IconButton
					icon="trash"
					label={m.rooms_delete()}
					inline
					hoverDanger
					size={13}
					disabled={working}
					onclick={() => house(() => deleteRoom(room.id))}
				/>
			{/if}
		</div>
	{/each}

	{#if filtered.length === 0 && !offerCreate}
		<EmptyState title={m.rooms_none()} />
	{/if}

	{#if onclear && current}
		<Button
			variant="ghost"
			size="sm"
			block
			style={ROW}
			disabled={busy || working}
			onclick={onclear}
		>
			<span class="glyph" aria-hidden="true">×</span>
			<span class="rowname">{m.rooms_clear()}</span>
		</Button>
	{/if}

	<Text size="xs" tone="muted">{m.rooms_menu_hint()}</Text>
</div>

<style>
	.menu {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		min-width: 14rem;
	}
	.find {
		display: flex;
	}
	.row {
		display: flex;
		gap: var(--sp-1);
		align-items: center;
		min-width: 0;
	}
	.glyph {
		flex: none;
		width: 1em;
		text-align: center;
	}
	.rowname {
		min-width: 0;
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
</style>
