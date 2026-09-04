<script lang="ts">
	// Machine listbox: a badge-shaped trigger tinted with the selected
	// machine's hue (same recipe as MachineBadge) opening a WAI-ARIA listbox
	// with a coloured dot + name + liveness per machine.
	import type { MachineRow } from '@bindings/MachineRow';
	import { hashHue } from '$lib/format';
	import { Dot, Icon, Popover } from '@dorsk/tsumikit';
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

	const liveStatus = (l: MachineRow['liveness']): 'active' | 'stale' | 'dead' =>
		l === 'online' ? 'active' : l === 'stale' ? 'stale' : 'dead';
	const liveLabel = (l: MachineRow['liveness']): string =>
		l === 'online'
			? m.dispatch_liveness_online()
			: l === 'stale'
				? m.dispatch_liveness_stale()
				: m.dispatch_liveness_offline();

	let listEl = $state<HTMLDivElement | null>(null);
	let active = $state(0);

	function options(): HTMLElement[] {
		return listEl ? Array.from(listEl.querySelectorAll<HTMLElement>('[role="option"]')) : [];
	}
	function focusAt(i: number) {
		const o = options();
		if (!o.length) return;
		active = (i + o.length) % o.length;
		o[active].focus();
	}
	function close() {
		listEl?.closest<HTMLElement>('[popover]')?.hidePopover();
	}
	function pick(id: string) {
		value = id;
		close();
	}
	function onopen() {
		const i = machines.findIndex((mc) => mc.id === value);
		queueMicrotask(() => focusAt(i < 0 ? 0 : i));
	}
	function onkeydown(e: KeyboardEvent) {
		switch (e.key) {
			case 'ArrowDown':
				e.preventDefault();
				focusAt(active + 1);
				break;
			case 'ArrowUp':
				e.preventDefault();
				focusAt(active - 1);
				break;
			case 'Home':
				e.preventDefault();
				focusAt(0);
				break;
			case 'End':
				e.preventDefault();
				focusAt(machines.length - 1);
				break;
			case 'Enter':
			case ' ': {
				e.preventDefault();
				const mc = machines[active];
				if (mc) pick(mc.id);
				break;
			}
		}
	}
</script>

<span class="picker" style={triggerTint}>
	<Popover {label} placement="bottom-start" bare triggerClass="machine-trigger" disabled={!machines.length} {onopen}>
		{#snippet trigger()}
			<span class="trigger-label">{selected ? nameOf(selected) : m.spawn_no_machines()}</span>
			<Icon name="chevron-down" size={12} />
		{/snippet}
		<div
			bind:this={listEl}
			role="listbox"
			aria-label={label}
			class="list"
			tabindex="-1"
			{onkeydown}
		>
			{#each machines as mc, i (mc.id)}
				<!-- svelte-ignore a11y_click_events_have_key_events -->
				<div
					role="option"
					class="option"
					aria-selected={mc.id === value}
					tabindex={i === active ? 0 : -1}
					onclick={() => pick(mc.id)}
					onfocus={() => (active = i)}
				>
					<Dot color="hsl({hueOf(mc)} var(--mach-fg-sl))" />
					<span class="name">{nameOf(mc)}</span>
					<Dot status={liveStatus(mc.liveness)} label={liveLabel(mc.liveness)} />
				</div>
			{/each}
		</div>
	</Popover>
</span>

<style>
	.picker {
		display: inline-flex;
		flex: none;
		border: 1px solid transparent;
		border-radius: var(--r-sm);
		font-size: var(--fs-xs);
		font-weight: var(--fw-medium);
	}
	.picker :global(.machine-trigger) {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		min-height: 0;
		min-width: 0;
		padding: 2px var(--sp-1) 2px var(--sp-2);
		border: 0;
		border-radius: inherit;
		background: transparent;
		color: inherit;
		font: inherit;
		cursor: pointer;
	}
	.picker :global(.machine-trigger:hover:not(:disabled)) {
		background: transparent;
		filter: brightness(1.08);
	}
	.picker :global(.machine-trigger:focus-visible) {
		outline: 2px solid currentColor;
		outline-offset: 1px;
	}
	.trigger-label {
		max-width: 12rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.list {
		display: flex;
		flex-direction: column;
		gap: 1px;
		min-width: 14rem;
		outline: none;
	}
	.option {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		border-radius: var(--r-sm);
		font-size: var(--fs-sm);
		color: var(--text);
		cursor: pointer;
		white-space: nowrap;
	}
	.option:hover,
	.option:focus-visible {
		background: var(--bg-elevated-2);
		outline: none;
	}
	.option[aria-selected='true'] {
		font-weight: var(--fw-semibold);
	}
	.name {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
	}
</style>
