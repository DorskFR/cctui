<script lang="ts">
	import type { Snippet } from 'svelte';
	import { IconButton } from '@dorsk/tsumikit';
	import DragGrip from '$lib/components/atoms/DragGrip.svelte';
	import { m } from '$lib/paraglide/messages';

	let {
		id,
		name,
		chain,
		usage = '',
		selected,
		open,
		position,
		total,
		dragging = false,
		dropTarget = false,
		onselect,
		ontoggle,
		onmove,
		ongrab,
		onover,
		children
	}: {
		id: string;
		name: string;
		chain: string;
		usage?: string;
		selected: boolean;
		open: boolean;
		position: number;
		total: number;
		dragging?: boolean;
		dropTarget?: boolean;
		onselect: () => void;
		ontoggle: () => void;
		onmove?: (delta: -1 | 1) => void;
		ongrab?: (e: PointerEvent) => void;
		onover?: () => void;
		children?: Snippet;
	} = $props();
</script>

<div
	class="profile"
	class:selected
	class:dragging
	class:drop-target={dropTarget}
	role="group"
	aria-label={name}
	data-profile-id={id}
	onpointermove={() => onover?.()}
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
				{#if usage}<span class="use" title={usage}>{usage}</span>{/if}
			</span>
			<span class="chain truncate" title={chain}>{chain}</span>
		</label>
		{#if onmove}
			<DragGrip
				label={m.spawn_profile_reorder({ name, position, total })}
				hint={m.spawn_profile_reorder_hint()}
				onmove={(delta) => onmove?.(delta)}
				{ongrab}
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
		--grip-opacity: 0.4;
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-2);
	}
	.head:hover,
	.head:focus-within {
		--grip-opacity: 1;
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
