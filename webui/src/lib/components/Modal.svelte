<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		title,
		onclose,
		body,
		footer
	}: {
		title: string;
		onclose: () => void;
		body: Snippet;
		footer?: Snippet;
	} = $props();

	function onkey(e: KeyboardEvent) {
		if (e.key === 'Escape') onclose();
	}
</script>

<svelte:window onkeydown={onkey} />

<div
	class="overlay"
	role="presentation"
	onclick={(e) => {
		if (e.target === e.currentTarget) onclose();
	}}
>
	<div class="sheet" role="dialog" aria-modal="true" aria-label={title}>
		<div class="sheet-head">
			<span class="sheet-title truncate">{title}</span>
			<div class="spacer"></div>
			<button class="btn btn-ghost btn-icon" aria-label="Close" onclick={onclose}>✕</button>
		</div>
		<div class="sheet-body">
			{@render body()}
		</div>
		{#if footer}
			<div class="sheet-foot">{@render footer()}</div>
		{/if}
	</div>
</div>
