<script lang="ts">
	import type { Snippet } from 'svelte';
	import { IconButton } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { draggedProfile, setDraggedProfile } from './profiles';

	let {
		id,
		name,
		chain,
		usage = '',
		selected,
		open,
		first = false,
		last = false,
		dragging = false,
		dropTarget = false,
		onselect,
		ontoggle,
		onmove,
		ondropped,
		onsourcechange,
		onover,
		children
	}: {
		id: string;
		name: string;
		chain: string;
		usage?: string;
		selected: boolean;
		open: boolean;
		first?: boolean;
		last?: boolean;
		dragging?: boolean;
		dropTarget?: boolean;
		onselect: () => void;
		ontoggle: () => void;
		onmove?: (delta: -1 | 1) => void;
		ondropped?: (targetId: string) => void;
		onsourcechange?: (sourceId: string) => void;
		onover?: (overId: string) => void;
		children?: Snippet;
	} = $props();

	const reorderable = $derived(Boolean(onmove));
</script>

<div
	class="profile"
	class:selected
	class:dragging
	class:drop-target={dropTarget}
	role="group"
	aria-label={name}
	draggable={reorderable}
	ondragstart={(e) => {
		setDraggedProfile(id);
		onsourcechange?.(id);
		e.dataTransfer?.setData('text/plain', id);
		if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move';
	}}
	ondragend={() => {
		setDraggedProfile('');
		onsourcechange?.('');
		onover?.('');
	}}
	ondragover={(e) => {
		if (!reorderable || !draggedProfile() || draggedProfile() === id) return;
		e.preventDefault();
		if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
		onover?.(id);
	}}
	ondrop={(e) => {
		const from = draggedProfile() || e.dataTransfer?.getData('text/plain') || '';
		if (!reorderable || !from || from === id) return;
		e.preventDefault();
		setDraggedProfile('');
		onsourcechange?.('');
		onover?.('');
		ondropped?.(id);
	}}
>
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
		{#if reorderable}
			<IconButton
				icon="chevron-up"
				label={m.spawn_profile_move_up({ name })}
				inline
				size={14}
				disabled={first}
				onclick={() => onmove?.(-1)}
			/>
			<IconButton
				icon="chevron-down"
				label={m.spawn_profile_move_down({ name })}
				inline
				size={14}
				disabled={last}
				onclick={() => onmove?.(1)}
			/>
		{/if}
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
	.profile.dragging {
		opacity: 0.5;
	}
	.profile.drop-target {
		border-color: var(--accent);
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
