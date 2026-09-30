<script lang="ts">
	import { m } from '$lib/paraglide/messages';
	import { Badge } from '@dorsk/tsumikit';
	import RoomBadge from '$lib/components/molecules/RoomBadge.svelte';
	import { roomIndex } from '$lib/roomIndex.svelte';
	import type { SessionView } from './view';

	// "N perm" (warn fill) · unread counter (danger fill) · ⚡ auto-approve · ◎ rooms.
	let { view }: { view: SessionView } = $props();
	const s = $derived(view.s);
	const rooms = $derived(roomIndex.for(s.id));
</script>

{#if view.pendingCount > 0}<Badge tone="warn" active size="xs" numeric style="flex:none"
		>{m.sessions_perm_count({ count: view.pendingCount })}</Badge
	>{/if}
{#if view.unreadCount > 0}<Badge
		tone="danger"
		active
		size="xs"
		numeric
		style="flex:none"
		title={m.sessions_unread_title({ count: view.unreadCount })}>{view.unreadCount}</Badge
	>{/if}
{#if s.auto_approve}<Badge tone="warn" size="xs" style="flex:none" title={m.sessions_auto_approve_title()}>⚡</Badge>{/if}
<RoomBadge {rooms} />
