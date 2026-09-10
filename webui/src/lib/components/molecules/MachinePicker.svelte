<script lang="ts">
	// Machine picker: a badge-shaped trigger tinted with the selected machine's
	// hue (same recipe as MachineBadge) with a native <Select variant="ghost">
	// laid over it, so the platform popup, keyboard and focus stay native.
	import type { MachineRow } from '@bindings/MachineRow';
	import { hashHue } from '$lib/format';
	import { Dot, Icon, Select, type SelectOption } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		value = $bindable(),
		machines,
		label
	}: {
		value: string;
		machines: MachineRow[];
		label: string;
	} = $props();

	const nameOf = (mc: MachineRow) => mc.display_name || mc.name;
	const hueOf = (mc: MachineRow) => mc.hue ?? hashHue(nameOf(mc));
	const selected = $derived(machines.find((mc) => mc.id === value));
	const triggerTint = $derived(
		selected
			? `--mh:${hueOf(selected)};` +
					'background:hsl(var(--mh) var(--mach-bg-sl));' +
					'color:hsl(var(--mh) var(--mach-fg-sl));' +
					'border-color:hsl(var(--mh) var(--mach-border-sl))'
			: ''
	);

	const liveLabel = (l: MachineRow['liveness']): string =>
		l === 'online'
			? m.dispatch_liveness_online()
			: l === 'stale'
				? m.dispatch_liveness_stale()
				: m.dispatch_liveness_offline();

	// Own id: the picker sits inside other components' <Field>, whose context
	// would otherwise hand this select the field's id and steal its label.
	const selectId = $props.id();

	const options = $derived<SelectOption[]>(
		machines.map((mc) => ({ value: mc.id, label: nameOf(mc), hint: liveLabel(mc.liveness) }))
	);
</script>

<span class="picker" class:disabled={!machines.length} style={triggerTint}>
	<span class="trigger" aria-hidden="true">
		{#if selected}
			<Dot color="hsl({hueOf(selected)} var(--mach-fg-sl))" />
		{/if}
		<span class="trigger-label">{selected ? nameOf(selected) : m.spawn_no_machines()}</span>
		<Icon name="chevron-down" size={12} />
	</span>
	<Select
		id={selectId}
		variant="ghost"
		aria-label={label}
		aria-describedby={null}
		disabled={!machines.length}
		{options}
		bind:value
	/>
</span>

<style>
	.picker {
		position: relative;
		display: inline-flex;
		flex: none;
		border: 1px solid transparent;
		border-radius: var(--r-sm);
		font-size: var(--fs-xs);
		font-weight: var(--fw-medium);
	}
	.picker:not(.disabled):hover {
		filter: brightness(1.08);
	}
	.picker:focus-within {
		outline: 2px solid currentColor;
		outline-offset: 1px;
	}
	.trigger {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		min-width: 0;
		padding: 3px var(--sp-2);
		border-radius: inherit;
	}
	.trigger-label {
		max-width: 12rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
</style>
