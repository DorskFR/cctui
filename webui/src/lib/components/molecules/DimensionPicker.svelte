<script lang="ts">
	import { Icon, Select } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { DIMENSIONS, type Dimension } from '../../../routes/sessions/sessions.logic';

	// Toolbar picker for a session-list dimension (CCT-466 color · CCT-467 group),
	// mirroring ViewPicker: a native <select> overlaid on a styled square trigger.
	let {
		value,
		onchange,
		kind,
		menu = false
	}: {
		value: Dimension;
		onchange: (v: Dimension) => void;
		kind: 'color' | 'group';
		/** Render as a full-width labeled row for the overflow ⋯ menu (icon + text),
		 * instead of the compact square toolbar trigger. */
		menu?: boolean;
	} = $props();

	const noun = $derived(kind === 'color' ? m.misc_color_noun() : m.misc_group_noun());
	const current = $derived(DIMENSIONS.find((d) => d.value === value)?.label ?? m.common_none());
	const active = $derived(value !== 'none');
	const title = $derived(
		active ? m.misc_dimension_by({ noun, value: current }) : m.misc_dimension_by_prompt({ noun })
	);
</script>

<div
	class="dim-picker {menu ? 'menu-row' : 'btn-control btn-control-square'}"
	class:active
	{title}
	aria-label={title}
>
	{#if kind === 'color'}
		<Icon size={18}>
			<circle cx="13.5" cy="6.5" r=".5" fill="currentColor" />
			<circle cx="17.5" cy="10.5" r=".5" fill="currentColor" />
			<circle cx="8.5" cy="7.5" r=".5" fill="currentColor" />
			<circle cx="6.5" cy="12.5" r=".5" fill="currentColor" />
			<path d="M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10c.926 0 1.648-.746 1.648-1.688 0-.437-.18-.835-.437-1.125-.29-.289-.438-.652-.438-1.125a1.64 1.64 0 0 1 1.668-1.668h1.996c3.051 0 5.555-2.503 5.555-5.554C21.965 6.012 17.461 2 12 2Z" />
		</Icon>
	{:else}
		<Icon size={18}>
			<rect x="3" y="4" width="18" height="5" rx="1" />
			<rect x="3" y="13" width="18" height="5" rx="1" />
		</Icon>
	{/if}
	<Select
		variant="ghost"
		aria-label={m.misc_dimension_sessions_by({ noun })}
		{value}
		onchange={(e) => onchange((e.currentTarget as HTMLSelectElement).value as Dimension)}
	>
		{#each DIMENSIONS as d (d.value)}
			<option value={d.value}>{m.misc_dimension_option({ noun, label: d.label })}</option>
		{/each}
	</Select>
</div>

<style>
	.dim-picker {
		position: relative;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		flex: none;
		white-space: nowrap;
		cursor: pointer;
	}
	/* A set dimension gets the accent tint so an active color/group is visible at
	   a glance without opening the popup. */
	.dim-picker.active {
		color: var(--accent);
	}
	/* Overflow-menu row: full-width, left-aligned icon + label (the aria-label,
	   e.g. "Group sessions by: Machine"), matching the drawer's ⋯ flyout rows.
	   The ghost <Select> still fills the row (inset:0), so the whole row opens the
	   native picker. */
	.dim-picker.menu-row {
		width: 100%;
		justify-content: flex-start;
		gap: var(--sp-2);
		min-height: 2.25rem;
		padding: var(--sp-1) var(--sp-2);
		border-radius: var(--r-sm);
		font-size: var(--fs-sm);
	}
	.dim-picker.menu-row:hover {
		background: var(--bg-elevated-3, var(--bg-elevated-2));
	}
	.dim-picker.menu-row::after {
		content: attr(aria-label);
		font-weight: var(--fw-medium);
		white-space: nowrap;
	}
</style>
