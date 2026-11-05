<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import RoomMenu from '$lib/components/molecules/RoomMenu.svelte';
	import KeepaliveModal from '$lib/components/molecules/KeepaliveModal.svelte';
	import CctuiverseInviteModal from '$lib/components/molecules/CctuiverseInviteModal.svelte';
	import CctuiverseJoinModal from '$lib/components/molecules/CctuiverseJoinModal.svelte';
	import { Button, Modal } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		session,
		title,
		keepaliveOpen = $bindable(false),
		roomOpen = $bindable(false),
		inviteOpen = $bindable(false),
		joinOpen = $bindable(false),
		onsetroom,
		onclearroom
	}: {
		session: SessionListItem;
		title: string;
		keepaliveOpen?: boolean;
		roomOpen?: boolean;
		inviteOpen?: boolean;
		joinOpen?: boolean;
		onsetroom?: (sessionId: string, pick: { id: string } | { name: string }) => void;
		onclearroom?: (sessionId: string) => void;
	} = $props();
</script>

{#if keepaliveOpen}
	<KeepaliveModal {session} onclose={() => (keepaliveOpen = false)} />
{/if}

{#if inviteOpen}
	<CctuiverseInviteModal
		target={{ session: session.id }}
		defaultLabel={title}
		onclose={() => (inviteOpen = false)}
	/>
{/if}

{#if joinOpen}
	<CctuiverseJoinModal sessionId={session.id} defaultLabel={title} onclose={() => (joinOpen = false)} />
{/if}

{#if roomOpen}
	<Modal title={m.rooms_menu_label()} onclose={() => (roomOpen = false)}>
		{#snippet body()}
			<RoomMenu
				current={session.room_id ?? null}
				onpick={(pick) => {
					onsetroom?.(session.id, pick);
					roomOpen = false;
				}}
				onclear={session.room_id
					? () => {
							onclearroom?.(session.id);
							roomOpen = false;
						}
					: undefined}
			/>
		{/snippet}
		{#snippet footer()}
			<Button size="sm" variant="ghost" onclick={() => (roomOpen = false)}>{m.common_close()}</Button>
		{/snippet}
	</Modal>
{/if}
