<script lang="ts">
	import { toasts } from '$lib/toast.svelte';
	import Card from '$lib/components/atoms/Card.svelte';
</script>

{#if toasts.items.length}
	<div class="toast-wrap">
		{#each toasts.items as t (t.id)}
			<Card
				as="button"
				class="toast {t.kind === 'err' ? 'toast-err' : ''} {t.kind === 'ok' ? 'toast-ok' : ''}"
				onclick={() => toasts.dismiss(t.id)}
			>
				{t.text}
			</Card>
		{/each}
	</div>
{/if}
