<script lang="ts">
	import { Cluster, Text, Timestamp } from '@dorsk/tsumikit';
	import Badges from './Badges.svelte';
	import DraftActions from './DraftActions.svelte';
	import Lead from './Lead.svelte';
	import Readout from './Readout.svelte';
	import type { SessionActions, SessionView } from './view';

	// One real row, no wrap: lead · preview (takes the slack) · perm · unread ·
	// Σ $ · model · effort · logo · time.
	let { view, actions }: { view: SessionView; actions: SessionActions } = $props();
	const s = $derived(view.s);
</script>

<Cluster wrap={false} gap="var(--sp-2)">
	<Lead {view} {actions} row />
	{#if s.match_snippet || view.lastMsg}
		<Text
			truncate
			tone={s.match_snippet ? 'default' : 'muted'}
			size="xs"
			style="flex:1 1 0;min-width:0"
			>{s.match_snippet ? `🔍 ${s.match_snippet}` : view.lastMsg}</Text
		>
	{:else}
		<span style="flex:1 1 auto"></span>
	{/if}
	<Badges {view} />
	{#if view.draft}
		<DraftActions {view} {actions} />
	{:else}
		<Readout {view} compact />
	{/if}
	{#if s.last_message_at}<span class="time"
			><Timestamp value={s.last_message_at} mode="relative" tone="faint" size="xs" /></span
		>{/if}
</Cluster>

<style>
	.time {
		flex: none;
		min-width: 30px;
		text-align: right;
		white-space: nowrap;
	}
</style>
