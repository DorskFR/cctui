<script lang="ts">
	import { Select, Toggle } from '@dorsk/tsumikit';
	import { useMachineResources } from '$lib/queries';
	import { DEFAULT_FILTERS, type EventFilters, type EventFamily, type EventSeverity } from '$lib/events';
	import { m } from '$lib/paraglide/messages';

	let { filters = $bindable(), admin = false }: { filters: EventFilters; admin?: boolean } = $props();

	const machines = useMachineResources(() => true);
	const machineOptions = $derived([
		{ value: '', label: m.events_filter_all_machines() },
		...(machines.data ?? []).map((r) => ({ value: r.machine_id, label: r.display_name || r.name }))
	]);

	const families: { id: 'all' | EventFamily; label: () => string }[] = [
		{ id: 'all', label: m.events_filter_all },
		{ id: 'session', label: m.events_family_session },
		{ id: 'machine', label: m.events_family_machine },
		{ id: 'system', label: m.events_family_system }
	];
	const severities: { id: 'all' | EventSeverity; label: () => string }[] = [
		{ id: 'all', label: m.events_filter_all },
		{ id: 'info', label: m.events_severity_info },
		{ id: 'warn', label: m.events_severity_warn },
		{ id: 'error', label: m.events_severity_error }
	];
	const shownFamilies = $derived(admin ? families : families.filter((f) => f.id !== 'system'));
</script>

<div class="bar" role="group" aria-label={m.events_filters_aria()}>
	<span class="group" role="group" aria-label={m.events_filter_family_aria()}>
		{#each shownFamilies as f (f.id)}
			<Toggle pill size="sm" pressed={filters.family === f.id} onclick={() => (filters = { ...filters, family: f.id })}>
				{f.label()}
			</Toggle>
		{/each}
	</span>
	<span class="group" role="group" aria-label={m.events_filter_severity_aria()}>
		{#each severities as s (s.id)}
			<Toggle pill size="sm" pressed={filters.severity === s.id} onclick={() => (filters = { ...filters, severity: s.id })}>
				{s.label()}
			</Toggle>
		{/each}
	</span>
	<Select
		size="sm"
		width="auto"
		aria-label={m.events_filter_machine_aria()}
		options={machineOptions}
		value={filters.machineId ?? ''}
		onchange={(e) => (filters = { ...filters, machineId: (e.currentTarget as HTMLSelectElement).value || null })}
	/>
	{#if filters.family !== DEFAULT_FILTERS.family || filters.severity !== DEFAULT_FILTERS.severity || filters.machineId}
		<Toggle size="sm" onclick={() => (filters = { ...DEFAULT_FILTERS })}>{m.events_filter_reset()}</Toggle>
	{/if}
</div>

<style>
	.bar {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2) var(--sp-3);
	}
	.group {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
	}
</style>
