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
		tabbable = false,
		onselect,
		ontoggle,
		onmove,
		ongrab,
		onover,
		onnav,
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
		tabbable?: boolean;
		onselect: () => void;
		ontoggle: () => void;
		onmove?: (delta: -1 | 1) => void;
		ongrab?: (e: PointerEvent) => void;
		onover?: () => void;
		onnav?: (key: string) => boolean;
		children?: Snippet;
	} = $props();

	// The grip and the gear are buttons inside the row, and the adjust panel is a
	// form: only a click landing on the label region may change the selection.
	function click(e: MouseEvent) {
		if ((e.target as HTMLElement | null)?.closest('.body')) onselect();
	}

	function keydown(e: KeyboardEvent) {
		if (e.target !== e.currentTarget) return;
		if (e.key === ' ' || e.key === 'Enter') {
			e.preventDefault();
			onselect();
			return;
		}
		if (onnav?.(e.key)) e.preventDefault();
	}
</script>

<div
	class="profile"
	class:selected
	class:dragging
	class:drop-target={dropTarget}
	role="radio"
	aria-checked={selected}
	aria-label={name}
	aria-describedby="sp-profile-chain-{id}"
	tabindex={tabbable ? 0 : -1}
	data-profile-id={id}
	onpointermove={() => onover?.()}
	onkeydown={keydown}
	onclick={click}
>
	<div class="head">
		{#if onmove}
			<DragGrip
				label={m.spawn_profile_reorder({ name, position, total })}
				hint={m.spawn_profile_reorder_hint()}
				onmove={(delta) => onmove?.(delta)}
				{ongrab}
			/>
		{/if}
		<span class="body">
			<span class="name">
				<span class="truncate">{name}</span>
				{#if usage}<span class="use" title={usage}>{usage}</span>{/if}
			</span>
			<span class="chain truncate" id="sp-profile-chain-{id}" title={chain}>{chain}</span>
		</span>
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
	/* Selection chrome matches tsumikit's OptionButton, which is what the run-target
	   switch above these rows uses. */
	.profile.selected {
		--oc: var(--accent);
		border-color: var(--oc);
		background: color-mix(in srgb, var(--oc) 14%, var(--bg));
		color: var(--oc);
	}
	.profile.selected:hover {
		background: color-mix(in srgb, var(--oc) 20%, var(--bg));
	}
	.profile.selected .chain,
	.profile.selected .use {
		color: color-mix(in srgb, var(--oc) 70%, var(--text-muted));
	}
	.profile:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: -2px;
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
		gap: var(--sp-1);
		padding: var(--sp-2);
		/* The grip is the row's leading gutter, so it supplies the inset itself. */
		padding-inline-start: var(--sp-1);
	}
	.head:hover,
	.head:focus-within {
		--grip-opacity: 1;
	}
	.body {
		min-width: 0;
		flex: 1;
		display: flex;
		flex-direction: column;
		gap: 2px;
		cursor: pointer;
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
