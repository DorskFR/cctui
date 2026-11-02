<script lang="ts">
	// Rendered inside RichSelect's trigger button and its listbox rows, so it
	// must stay free of controls.
	import AccountBadge from './AccountBadge.svelte';
	import type { BadgeLook } from './accountBadge';

	let {
		id,
		name,
		chain,
		badge,
		variant = 'row'
	}: {
		id: string;
		name: string;
		chain: string;
		badge: BadgeLook;
		variant?: 'card' | 'row' | 'draft';
	} = $props();
</script>

<span class="profile {variant}" data-profile-id={id} data-journey="profile" data-journey-key={id}>
	<AccountBadge look={badge} size={variant === 'row' ? 'row' : 'card'} />
	<span class="text">
		<span class="name truncate">{name}</span>
		<span class="chain truncate" title={chain}>{chain}</span>
	</span>
</span>

<style>
	.profile {
		min-width: 0;
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		user-select: none;
	}
	.text {
		min-width: 0;
		flex: 1 1 auto;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.name {
		font-size: var(--fs-sm);
		font-weight: var(--fw-semibold);
	}
	.chain {
		font-size: var(--fs-xs);
		color: var(--text-muted);
	}
	.card .name {
		color: var(--accent);
	}
	.card .chain {
		color: color-mix(in srgb, var(--accent) 70%, var(--text-muted));
	}
	.draft .name {
		color: var(--text-muted);
	}
	.truncate {
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
</style>
