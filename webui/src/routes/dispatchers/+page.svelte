<script lang="ts">
	// Dispatcher management now lives under Accounts (CCT-403); this route stays
	// as a deep-linkable wrapper around the shared panel.
	import DispatchersPanel from '$lib/components/organisms/DispatchersPanel.svelte';
</script>

<DispatchersPanel />
