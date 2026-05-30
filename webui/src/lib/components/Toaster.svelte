<script lang="ts">
	import { toasts } from '$lib/toast.svelte';
</script>

{#if toasts.items.length}
	<div class="toast-wrap">
		{#each toasts.items as t (t.id)}
			<button
				class="toast"
				class:toast-err={t.kind === 'err'}
				class:toast-ok={t.kind === 'ok'}
				onclick={() => toasts.dismiss(t.id)}
			>
				{t.text}
			</button>
		{/each}
	</div>
{/if}

<style>
	.toast {
		text-align: left;
		width: 100%;
		color: inherit;
	}
</style>
