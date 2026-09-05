<script lang="ts">
	import type { Snippet } from 'svelte';
	import { IconButton } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		id,
		name,
		chain,
		usage = '',
		selected,
		open,
		onselect,
		ontoggle,
		children
	}: {
		id: string;
		name: string;
		chain: string;
		usage?: string;
		selected: boolean;
		open: boolean;
		onselect: () => void;
		ontoggle: () => void;
		children?: Snippet;
	} = $props();
</script>

<div class="profile" class:selected>
	<div class="head">
		<input
			class="radio"
			type="radio"
			name="spawn-profile"
			id="sp-profile-{id}"
			value={id}
			checked={selected}
			onchange={onselect}
		/>
		<label class="body" for="sp-profile-{id}">
			<span class="name">
				<span class="truncate">{name}</span>
				{#if usage}<span class="use">{usage}</span>{/if}
			</span>
			<span class="chain truncate" title={chain}>{chain}</span>
		</label>
		<IconButton
			icon="settings"
			label={m.spawn_profile_adjust()}
			inline
			size={14}
			pressed={open}
			aria-expanded={open}
			onclick={ontoggle}
		/>
	</div>
	{#if open}{@render children?.()}{/if}
</div>

<style>
	.profile {
		border: 1px solid var(--border);
		border-radius: var(--r-md);
		background: var(--bg);
		overflow: hidden;
	}
	.profile.selected {
		border-color: var(--accent-dim);
	}
	.head {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-2);
	}
	.radio {
		flex: none;
		width: 1rem;
		height: 1rem;
		margin: 0;
		accent-color: var(--accent);
		cursor: pointer;
	}
	.body {
		min-width: 0;
		flex: 1;
		display: flex;
		flex-direction: column;
		gap: 2px;
		cursor: pointer;
	}
	.name {
		display: flex;
		justify-content: space-between;
		align-items: baseline;
		gap: var(--sp-2);
		font-size: var(--fs-sm);
		font-weight: var(--fw-semibold);
	}
	.use {
		flex: none;
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
