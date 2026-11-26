<script lang="ts">
	// A trigger tinted with the selected machine's hue (MachineBadge recipe)
	// under a native <Select variant="ghost">, so popup, keyboard and focus stay
	// native. `pill` is the bare badge for inline prefixes; `field` is an
	// outlined select with the badge inside.
	import type { MachineResourcesRow } from '@bindings/MachineResourcesRow';
	import type { MachineRow } from '@bindings/MachineRow';
	import { hashHue } from '#lib/format.ts';
	import { Dot, Icon, Select, type SelectOption } from '@dorsk/tsumikit';
	import { m } from '#lib/paraglide/messages.js';

	let {
		value = $bindable(),
		machines,
		label,
		variant = 'pill'
	}: {
		value: string;
		machines: (MachineResourcesRow | MachineRow)[];
		label: string;
		variant?: 'pill' | 'field';
	} = $props();

	const idOf = (mc: MachineResourcesRow | MachineRow) =>
		'machine_id' in mc ? mc.machine_id : mc.id;
	const nameOf = (mc: MachineResourcesRow | MachineRow) => mc.display_name || mc.name;
	const hueOf = (mc: MachineResourcesRow | MachineRow) => mc.hue ?? hashHue(nameOf(mc));
	const selected = $derived(machines.find((mc) => idOf(mc) === value));
	const triggerTint = $derived(
		selected
			? `--mh:${hueOf(selected)};` +
					'background:hsl(var(--mh) var(--mach-bg-sl));' +
					'color:hsl(var(--mh) var(--mach-fg-sl));' +
					'border-color:hsl(var(--mh) var(--mach-border-sl))'
			: ''
	);

	const liveLabel = (l: MachineResourcesRow['liveness'] | MachineRow['liveness']): string =>
		l === 'online'
			? m.dispatch_liveness_online()
			: l === 'stale'
				? m.dispatch_liveness_stale()
				: m.dispatch_liveness_offline();

	// Own id: the picker sits inside other components' <Field>, whose context
	// would otherwise hand this select the field's id and steal its label.
	const selectId = $props.id();

	const options = $derived<SelectOption[]>(
		machines.map((mc) => ({ value: idOf(mc), label: nameOf(mc), hint: liveLabel(mc.liveness) }))
	);
</script>

{#snippet badge()}
	{#if selected}
		<Dot color="hsl({hueOf(selected)} var(--mach-fg-sl))" />
	{/if}
	<span class="trigger-label">{selected ? nameOf(selected) : m.spawn_no_machines()}</span>
{/snippet}

<span
	class="picker"
	class:field={variant === 'field'}
	class:disabled={!machines.length}
	style={variant === 'pill' ? triggerTint : undefined}
>
	<span class="trigger" aria-hidden="true">
		{#if variant === 'field'}
			<span class="pill" class:tinted={!!selected} style={triggerTint}>{@render badge()}</span>
			<Icon name="chevron-down" size={14} />
		{:else}
			{@render badge()}
			<Icon name="chevron-down" size={12} />
		{/if}
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
	.picker.field {
		display: flex;
		min-height: var(--control-height-default);
		background: var(--control-fill);
		border: var(--edge-width) solid var(--control-edge);
		border-radius: var(--r-md);
		box-shadow: var(--control-shadow);
		color: var(--text-faint);
		font-weight: var(--fw-normal);
	}
	.picker.field:not(.disabled):hover {
		filter: none;
		border-color: var(--text-faint);
	}
	.picker.field:focus-within {
		outline: var(--focus-ring);
		outline-offset: var(--focus-ring-offset);
		border-color: var(--accent);
	}
	.field .trigger {
		flex: 1;
		justify-content: space-between;
		gap: var(--sp-2);
		padding: var(--sp-1) var(--control-px-md) var(--sp-1) var(--sp-2);
	}
	.pill {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		min-width: 0;
		padding: 2px var(--sp-2);
		border: 1px solid transparent;
		border-radius: var(--r-sm);
		color: var(--text);
		font-size: var(--fs-xs);
		font-weight: var(--fw-medium);
	}
	.trigger-label {
		max-width: 12rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
</style>
