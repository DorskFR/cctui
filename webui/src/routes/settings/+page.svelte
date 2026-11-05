<script lang="ts">
	// `/settings` forwards to the first area, and an older `/settings#anchor`
	// link to wherever that anchor lives now.
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { settingsLocationForHash } from '$lib/components/organisms/settings/settings.logic';

	onMount(() => {
		void goto(settingsLocationForHash(location.hash), { replaceState: true });
	});
</script>
