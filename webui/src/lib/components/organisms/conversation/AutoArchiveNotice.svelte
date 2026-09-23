<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { Button, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { getLocale } from '$lib/paraglide/runtime';

	let {
		session,
		onpin
	}: {
		session: Pick<SessionListItem, 'status' | 'liveness' | 'auto_archive_at' | 'archived_by'>;
		onpin: () => void;
	} = $props();

	const archivedAutomatically = $derived(
		session.status === 'archived' && session.archived_by === 'automatic'
	);
	const due = $derived(
		session.status !== 'archived' && session.liveness !== 'active' ? session.auto_archive_at : null
	);
	const dueLabel = $derived(
		due
			? new Date(due).toLocaleString(getLocale(), { dateStyle: 'medium', timeStyle: 'short' })
			: ''
	);
</script>

{#if archivedAutomatically}
	<div class="notice" role="note" data-testid="auto-archive-notice">
		<Text as="span" tone="faint" size="xs">{m.auto_archive_done()}</Text>
	</div>
{:else if due}
	<div class="notice" role="note" data-testid="auto-archive-notice">
		<Text as="span" tone="faint" size="xs">{m.auto_archive_due({ at: dueLabel })}</Text>
		<Button size="sm" variant="ghost" onclick={onpin}>{m.auto_archive_keep()}</Button>
	</div>
{/if}

<style>
	.notice {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		padding: 0 var(--sp-3);
		flex: none;
	}
</style>
