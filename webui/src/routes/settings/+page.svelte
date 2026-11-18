<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { hrefForHash } from '#lib/components/organisms/settings/settings.logic.ts';

	onMount(() => {
		void goto(hrefForHash(location.hash), { replace: true });
	});
</script>
