<script lang="ts">
	// Machine picker: an outlined field like every other select, with the chosen
	// machine as a hue-tinted pill inside (same recipe as MachineBadge). A native
	// <Select variant="ghost"> is laid over it, so the platform popup, keyboard
	// and focus stay native.
	import type { MachineResourcesRow } from '@bindings/MachineResourcesRow';
	import type { MachineRow } from '@bindings/MachineRow';
	import { hashHue } from '$lib/format';
	import { Dot, Icon, Select, type SelectOption } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		value = $bindable(),
		machines,
		label,
		onchange
	}: {
		value: string;
		machines: (MachineResourcesRow | MachineRow)[];
		label: string;
		/** After the user picked a machine (`value` is already updated). */
		onchange?: (value: string) => void;
	} = $props();

	const idOf = (mc: MachineResourcesRow | MachineRow) =>
		'machine_id' in mc ? mc.machine_id : mc.id;
	const nameOf = (mc: MachineResourcesRow | MachineRow) => mc.display_name || mc.name;
	const hueOf = (mc: MachineResourcesRow | MachineRow) => mc.hue ?? hashHue(nameOf(mc));
	const selected = $derived(machines.find((mc) => idOf(mc) === value));
	const pillTint = $derived(selected ? `--mh:${hueOf(selected)}` : '');

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

<span class="picker" class:disabled={!machines.length}>
	<span class="trigger" aria-hidden="true">
		{#if selected}
			<span class="pill" style={pillTint}>
				<Dot color="hsl(var(--mh) var(--mach-fg-sl))" />
				<span class="pill-label">{nameOf(selected)}</span>
			</span>
		{:else}
			<span class="empty">{m.spawn_no_machines()}</span>
		{/if}
		<span class="chev"><Icon name="chevron-down" size={14} /></span>
	</span>
	<Select
		id={selectId}
		variant="ghost"
		aria-label={label}
		aria-describedby={null}
		disabled={!machines.length}
		{options}
		bind:value
		onchange={() => onchange?.(value)}
	/>
</span>

<style>
	.picker {
		position: relative;
		display: inline-flex;
		flex: none;
		min-width: 12rem;
		max-width: 100%;
		min-height: var(--control-height-default);
		border: 1px solid var(--border);
		border-radius: var(--r-md);
		background: var(--bg);
		color: var(--text);
		font-size: var(--fs-sm);
	}
	.picker:not(.disabled):hover {
		border-color: var(--border-strong, var(--text-faint));
	}
	.picker:focus-within {
		outline: 2px solid var(--accent);
		outline-offset: 1px;
	}
	.trigger {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		width: 100%;
		min-width: 0;
		padding: 0 var(--sp-2);
	}
	.pill {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		min-width: 0;
		padding: 2px var(--sp-2);
		border-radius: var(--r-sm);
		background: hsl(var(--mh) var(--mach-bg-sl));
		color: hsl(var(--mh) var(--mach-fg-sl));
		font-size: var(--fs-xs);
		font-weight: var(--fw-semibold);
	}
	.pill-label {
		max-width: 12rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.empty {
		color: var(--text-faint);
	}
	.chev {
		display: inline-flex;
		margin-left: auto;
		color: var(--text-faint);
	}
</style>
