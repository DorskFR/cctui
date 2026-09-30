<script lang="ts">
	import {
		Badge,
		Button,
		Callout,
		ConfirmModal,
		Heading,
		Input,
		SectionHeader,
		Text
	} from '@dorsk/tsumikit';
	import { errMessage } from '$lib/api';
	import { m } from '$lib/paraglide/messages';
	import { toasts } from '$lib/toast.svelte';
	import RoomMemberList from '$lib/components/molecules/RoomMemberList.svelte';
	import RoomTimeline from '$lib/components/molecules/RoomTimeline.svelte';
	import {
		deleteRoom,
		getRoom,
		mergeMessages,
		nextCursor,
		postProblem,
		postToRoom,
		removeRoomMember,
		roomMessages,
		setRoomArchived,
		type Room,
		type RoomMember,
		type RoomMessage
	} from '$lib/rooms';

	let {
		room: initial,
		onchanged
	}: {
		room: Room;
		/** The room's own state changed (membership, archive, deletion) — reload. */
		onchanged?: () => void;
	} = $props();

	let room = $state<Room>(initial);
	let messages = $state<RoomMessage[]>([]);
	let draft = $state('');
	let busy = $state(false);
	let error = $state<string | null>(null);
	let confirming = $state(false);

	// The panel's own copy follows the prop when the parent switches rooms, and
	// the timeline is reloaded from scratch for the new id.
	$effect(() => {
		const id = initial.id;
		room = initial;
		messages = [];
		void reload(id);
	});

	const head = $derived(nextCursor(messages) ?? 0);
	const problem = $derived(postProblem(room, draft));

	async function reload(id: string) {
		try {
			const [fresh, page] = await Promise.all([getRoom(id), roomMessages(id)]);
			// A late reply for a room the user already navigated away from must not
			// overwrite the current one.
			if (id !== initial.id) return;
			room = fresh;
			messages = mergeMessages([], page);
			error = null;
		} catch (e) {
			error = errMessage(e);
		}
	}

	/** Pull whatever arrived since the newest post held. */
	export async function refresh() {
		try {
			const page = await roomMessages(room.id, nextCursor(messages));
			messages = mergeMessages(messages, page);
			room = await getRoom(room.id);
		} catch {
			// A failed catch-up is not worth a toast: the next event retries it.
		}
	}

	function problemMessage(p: NonNullable<typeof problem>): string {
		if (p === 'empty') return m.rooms_post_empty();
		if (p === 'too-large') return m.rooms_post_too_large();
		return m.rooms_post_archived();
	}

	async function post() {
		if (problem) {
			toasts.info(problemMessage(problem));
			return;
		}
		busy = true;
		try {
			const sent = await postToRoom(room.id, draft);
			messages = mergeMessages(messages, [sent]);
			draft = '';
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			busy = false;
		}
	}

	async function drop(mem: RoomMember) {
		busy = true;
		try {
			await removeRoomMember(room.id, mem.session_id);
			room = await getRoom(room.id);
			onchanged?.();
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			busy = false;
		}
	}

	async function remove() {
		busy = true;
		try {
			await deleteRoom(room.id);
			confirming = false;
			onchanged?.();
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			busy = false;
		}
	}

	async function toggleArchive() {
		busy = true;
		try {
			room = await setRoomArchived(room.id, !room.archived);
			onchanged?.();
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			busy = false;
		}
	}
</script>

<section class="panel">
	<header class="head">
		<Heading level={3} size="sm">{room.name}</Heading>
		{#if room.archived}
			<Badge size="xs" uppercase>{m.rooms_archived()}</Badge>
		{/if}
		<Text size="xs" tone="muted">{m.rooms_members_count({ count: room.members.length })}</Text>
		<div class="spacer"></div>
		<Button variant="ghost" size="sm" disabled={busy} onclick={toggleArchive}>
			{room.archived ? m.rooms_unarchive() : m.rooms_archive()}
		</Button>
		<Button variant="danger" size="sm" disabled={busy} onclick={() => (confirming = true)}>
			{m.rooms_delete()}
		</Button>
	</header>

	{#if confirming}
		<ConfirmModal
			open
			tone="danger"
			title={m.rooms_delete()}
			message={room.name}
			confirmLabel={m.rooms_delete()}
			onconfirm={remove}
			oncancel={() => (confirming = false)}
		/>
	{/if}

	{#if error}
		<Callout tone="danger">{error}</Callout>
	{/if}

	<SectionHeader title={m.rooms_title()} />
	<div class="scroll">
		<RoomTimeline {messages} />
	</div>

	<div class="composer">
		<Input
			bind:value={draft}
			disabled={busy || room.archived}
			placeholder={m.rooms_composer_placeholder()}
			onkeydown={(e: KeyboardEvent) => {
				if (e.key === 'Enter' && !e.shiftKey) {
					e.preventDefault();
					void post();
				}
			}}
		/>
		<Button
			loading={busy}
			disabled={busy || problem !== null}
			onclick={post}
			title={problem ? problemMessage(problem) : m.rooms_post()}
		>
			{m.rooms_post()}
		</Button>
	</div>

	<SectionHeader title={m.rooms_members_count({ count: room.members.length })} />
	<RoomMemberList members={room.members} {head} {busy} onRemove={drop} />
</section>

<style>
	.panel {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
		min-height: 0;
	}
	.head {
		display: flex;
		gap: var(--sp-2);
		align-items: center;
		min-width: 0;
	}
	.spacer {
		flex: 1;
	}
	/* The timeline is the only part that scrolls; the composer and the roster
	   stay put so posting never chases a moving target. */
	.scroll {
		flex: 1;
		min-height: 0;
		overflow-y: auto;
	}
	.composer {
		display: flex;
		gap: var(--sp-2);
		align-items: center;
	}
</style>
