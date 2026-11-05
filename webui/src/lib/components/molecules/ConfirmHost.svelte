<script lang="ts">
	// The one dialog behind `confirmDialog()`; mounted once in the root layout.
	import { ConfirmModal } from '@dorsk/tsumikit';
	import { confirmQueue, type ConfirmRequest } from '$lib/confirmDialog.svelte';

	const request = $derived(confirmQueue.current);
	let shown = $state.raw<ConfirmRequest | null>(null);
	let open = $state(false);

	$effect(() => {
		if (request) shown = request;
		open = request !== null;
	});

	$effect(() => () => confirmQueue.clear());
</script>

{#if shown}
	{@const req = shown}
	<ConfirmModal
		bind:open
		title={req.title}
		message={req.message}
		confirmLabel={req.confirmLabel}
		cancelLabel={req.cancelLabel}
		tone={req.tone}
		onconfirm={() => confirmQueue.settle(req.id, true)}
		oncancel={() => confirmQueue.settle(req.id, false)}
	/>
{/if}
