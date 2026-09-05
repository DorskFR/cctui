<script lang="ts">
	// `/settings` is not a page of its own any more; it forwards to the first
	// settings page, and an older `/settings#anchor` link to whichever page now
	// owns that anchor.
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import {
		pageForHash,
		settingsHref
	} from '$lib/components/organisms/settings/settings.logic';

	onMount(() => {
		void goto(settingsHref(pageForHash(location.hash)), { replaceState: true });
	});
</script>
