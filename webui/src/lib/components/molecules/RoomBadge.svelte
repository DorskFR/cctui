<script lang="ts">
	import { Badge } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	// A room is a field on the session, so the badge is just its name.
	let { name }: { name: string | null | undefined } = $props();
	const title = $derived(name ? m.rooms_badge_title({ room: name }) : '');
</script>

{#if name}
	<Badge size="xs" style="flex:none" {title}>◎ {name}</Badge>
{/if}
