<script lang="ts">
	import { Button } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		count,
		onfork,
		onforkall,
		oncancel
	}: {
		count: number;
		onfork: () => void;
		onforkall: () => void;
		oncancel: () => void;
	} = $props();
</script>

<div class="fork-select-bar row">
	{#if count > 0}
		<span class="fork-select-count">{count}</span>
	{/if}
	<Button variant="primary" onclick={onfork} disabled={count === 0}>
		{m.fork_selection()}
	</Button>
	<Button onclick={onforkall}>{m.drawer_fork_label()}</Button>
	<Button onclick={oncancel}>{m.common_cancel()}</Button>
</div>

<style>
	.fork-select-bar {
		position: fixed;
		bottom: 1rem;
		left: 50%;
		transform: translateX(-50%);
		z-index: 199;
		gap: var(--sp-2);
		align-items: center;
		flex-wrap: nowrap;
		padding: var(--sp-2) var(--sp-3);
		background: var(--bg-elevated-2);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		box-shadow: var(--shadow-lg);
		white-space: nowrap;
	}
	.fork-select-count {
		font-variant-numeric: tabular-nums;
		font-weight: 600;
		opacity: 0.85;
		padding-left: var(--sp-1);
	}
</style>
