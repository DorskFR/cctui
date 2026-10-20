<script lang="ts">
	import type { MachineRow } from '@bindings/MachineRow';
	import { Modal, Text } from '@dorsk/tsumikit';
	import EventRow from '../events/EventRow.svelte';
	import { useMachineEvents } from '$lib/queries';
	import { machineHistory } from '$lib/events';
	import { m } from '$lib/paraglide/messages';

	let { machine, onclose }: { machine: MachineRow; onclose: () => void } = $props();

	const events = useMachineEvents(() => machine.id);
	const rows = $derived(machineHistory(events.data?.events ?? []));
</script>

<Modal title={m.events_machine_history_title({ machine: machine.display_name || machine.name })} size="md" {onclose}>
	{#snippet body()}
		{#if events.isPending}
			<Text size="sm" tone="faint">{m.common_loading()}</Text>
		{:else if events.isError}
			<Text size="sm" tone="danger">{m.events_panel_error()}</Text>
		{:else if rows.length === 0}
			<Text size="sm" tone="faint">{m.events_machine_history_empty()}</Text>
		{:else}
			<ul class="list">
				{#each rows as ev (ev.id)}
					<EventRow event={ev} compact />
				{/each}
			</ul>
		{/if}
	{/snippet}
</Modal>

<style>
	.list {
		list-style: none;
		margin: 0;
		padding: 0;
		max-height: 60vh;
		overflow-y: auto;
	}
</style>
