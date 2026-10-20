<script lang="ts">
	import { Badge, Button, Card, Spinner, Text } from '@dorsk/tsumikit';
	import type { PendingSpawn } from '$lib/pendingSpawns.svelte';
	import { m } from '$lib/paraglide/messages';

	let {
		spawn,
		ondismiss
	}: {
		spawn: PendingSpawn;
		ondismiss: (commandId: string) => void;
	} = $props();

	const failed = $derived(spawn.failure !== null);
	const title = $derived(spawn.name.trim() || spawn.workingDir);
</script>

<Card padding="sm" data-journey="pending-spawn" data-failed={failed || undefined}>
	<div class="row" class:failed>
		<div class="lead">
			{#if failed}
				<Badge tone="danger" size="sm" border>{m.sessions_end_spawn_failed()}</Badge>
			{:else}
				<Spinner size={14} label={m.spawn_pending_waiting()} />
			{/if}
			<Text weight="bold" truncate>{title}</Text>
			<Text size="xs" tone="faint" variant="code" truncate>{spawn.workingDir}</Text>
		</div>
		<div class="body">
			{#if failed}
				<pre class="detail">{spawn.failure}</pre>
			{:else}
				<Text size="sm" tone="muted">{m.spawn_pending_waiting()}</Text>
			{/if}
		</div>
		<Button size="sm" variant={failed ? 'ghost' : 'link'} onclick={() => ondismiss(spawn.commandId)}>
			{m.spawn_pending_dismiss()}
		</Button>
	</div>
</Card>

<style>
	.row {
		display: flex;
		align-items: flex-start;
		gap: var(--sp-3);
		flex-wrap: wrap;
	}
	.lead {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
		flex: 1 1 16rem;
	}
	.body {
		flex: 2 1 20rem;
		min-width: 0;
	}
	.failed .body {
		flex-basis: 100%;
	}
	.detail {
		margin: 0;
		max-height: 12rem;
		overflow: auto;
		white-space: pre-wrap;
		font: var(--font-mono, monospace);
		font-size: var(--fs-xs, 0.75rem);
		color: var(--danger);
		user-select: text;
	}
</style>
