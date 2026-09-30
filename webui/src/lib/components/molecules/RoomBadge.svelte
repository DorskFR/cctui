<script lang="ts">
	import { Badge } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import type { SessionRoom } from '$lib/rooms';

	let { rooms }: { rooms: SessionRoom[] } = $props();

	const title = $derived(
		`${m.rooms_badge_title({ count: rooms.length })} — ${rooms.map((r) => r.name).join(', ')}`
	);
	const href = $derived(rooms.length === 1 ? `/rooms?room=${rooms[0].id}` : '/rooms');
</script>

{#if rooms.length > 0}
	<a class="link" {href} {title} aria-label={title} onclick={(e) => e.stopPropagation()}>
		<Badge size="xs" numeric style="flex:none">◎{rooms.length > 1 ? rooms.length : ''}</Badge>
	</a>
{/if}

<style>
	/* The card is itself clickable, so the badge swallows its own click rather
	   than opening the session behind it. */
	.link {
		display: inline-flex;
		flex: none;
		text-decoration: none;
	}
</style>
