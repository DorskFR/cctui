<script lang="ts">
	import { m } from '$lib/paraglide/messages';
	import { Badge } from '@dorsk/tsumikit';
	import CountChip from '$lib/components/atoms/CountChip.svelte';
	import ContextCountsBadge from '$lib/components/molecules/ContextCountsBadge.svelte';
	import RoomBadge from '$lib/components/molecules/RoomBadge.svelte';
	import { contextCountsOf, type SessionView } from './view';

	let { view, compact = false }: { view: SessionView; compact?: boolean } = $props();
	const s = $derived(view.s);
	const context = $derived(contextCountsOf(s));
</script>

{#if view.pendingCount > 0}<Badge tone="warn" active size="xs" numeric style="flex:none"
		>{m.sessions_perm_count({ count: view.pendingCount })}</Badge
	>{/if}
{#if view.unreadCount > 0}<span class="chip" title={m.sessions_unread_title({ count: view.unreadCount })}
		><CountChip count={view.unreadCount} label={m.sessions_unread_title({ count: view.unreadCount })} /></span
	>{/if}
{#if s.hidden}<Badge tone="muted" size="xs" style="flex:none" data-journey="hidden-chip" title={m.hidden_owners_chip_title()}
		>{m.hidden_owners_chip()}</Badge
	>{/if}
{#if s.auto_approve}<Badge tone="warn" size="xs" style="flex:none" title={m.sessions_auto_approve_title()}
		>{m.list_auto_approve()}</Badge
	>{/if}
{#if !compact}
	<RoomBadge name={s.room_name} />
	{#if context}<ContextCountsBadge counts={context} />{/if}
{/if}

<style>
	.chip {
		display: inline-flex;
		flex: none;
	}
</style>
