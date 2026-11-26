<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import RoomBadge from '#lib/components/molecules/RoomBadge.svelte';
	import CctuiverseLinkChips from '#lib/components/molecules/CctuiverseLinkChips.svelte';
	import { Icon, Input, Text } from '@dorsk/tsumikit';
	import HeaderLabels from './HeaderLabels.svelte';
	import type { LabelHandlers } from './headerLabels';
	import { headerTitleOf } from '#lib/sessionName.ts';
	import { m } from '#lib/paraglide/messages.js';

	let {
		session,
		compact,
		showLabels,
		labels,
		renaming,
		name = $bindable(),
		onsubmit,
		onrename
	}: {
		session: SessionListItem;
		compact: boolean;
		showLabels: boolean;
		labels: LabelHandlers;
		renaming: boolean;
		name: string;
		onsubmit: () => void;
		onrename: () => void;
	} = $props();
</script>

<div class="dtitle">
	{#if renaming}
		<Input bind:value={name} aria-label={m.a11y_rename_session()} {onsubmit} />
	{:else}
		<button type="button" class="tname" title={m.drawer_rename()} onclick={onrename} data-journey="rename">
			<Text weight="semibold" size="md" truncate>{headerTitleOf(session)}</Text>
			<span class="pen"><Icon name="edit" size={14} /></span>
		</button>
		{#if showLabels && session.labels.length === 0 && labels.onAttachLabel}
			<!-- No labels yet: the picker rides inline after the title; once
			     labels exist the strip moves to its own row below. -->
			<HeaderLabels {session} handlers={labels} />
		{/if}
		<RoomBadge name={session.room_name} />
		{#if !compact}
			<CctuiverseLinkChips target={{ session: session.id }} />
		{/if}
	{/if}
</div>

<style>
	.dtitle {
		flex: 1;
		min-width: 0;
		display: flex;
		align-items: center;
		gap: var(--sp-1);
	}
	.tname {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		min-width: 0;
		padding: 0;
		border: 0;
		background: none;
		color: inherit;
		font: inherit;
		text-align: start;
		cursor: text;
	}
	.pen {
		display: inline-flex;
		flex: none;
		color: var(--text-muted);
		opacity: 0;
	}
	.tname:hover .pen,
	.tname:focus-visible .pen {
		opacity: 1;
	}
</style>
