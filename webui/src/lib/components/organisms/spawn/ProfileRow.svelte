<script lang="ts">
	// Rendered inside RichSelect's trigger button and its listbox rows, so it
	// must stay free of controls.
	let {
		id,
		name,
		chain,
		usage = ''
	}: {
		id: string;
		name: string;
		chain: string;
		usage?: string;
	} = $props();
</script>

<span class="profile" data-profile-id={id} data-journey="profile" data-journey-key={id}>
	<span class="name">
		<span class="truncate">{name}</span>
		{#if usage}<span class="use" title={usage}>{usage}</span>{/if}
	</span>
	<span class="chain truncate" title={chain}>{chain}</span>
</span>

<style>
	.profile {
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
		padding: var(--sp-0-5) var(--sp-1);
		user-select: none;
	}
	.name {
		display: flex;
		justify-content: space-between;
		align-items: baseline;
		gap: var(--sp-2);
		font-size: var(--fs-sm);
		font-weight: var(--fw-semibold);
	}
	/* The name outranks the usage chip: the chip absorbs the squeeze first and
	   ellipsises, and the name only shrinks once it is down to its floor. */
	.name > .truncate {
		flex: 1 1 auto;
		min-width: 10ch;
	}
	.use {
		flex: 0 1 auto;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		font-size: var(--fs-xs);
		font-weight: var(--fw-normal);
		color: var(--text-faint);
		white-space: nowrap;
	}
	.chain {
		font-size: var(--fs-xs);
		color: var(--text-muted);
	}
	.truncate {
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
</style>
