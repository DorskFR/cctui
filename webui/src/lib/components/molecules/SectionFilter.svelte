<script lang="ts">
	import { Badge, Icon, Menu, type MenuItem } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { SECTIONS, type Section } from '../../../routes/sessions/sessions.logic';

	// One square toolbar button that opens a menu of INDEPENDENT on/off section
	// toggles — any combination can be shown at once, so every item is `keepOpen`.
	// `sections` is bindable so the parent owns persistence; toggling mutates it
	// here (never to empty).
	let { sections = $bindable() }: { sections: Set<Section> } = $props();

	const count = $derived(sections.size);
	const filtering = $derived(count < SECTIONS.length);

	function toggle(v: Section) {
		const next = new Set(sections);
		if (next.has(v)) next.delete(v);
		else next.add(v);
		if (next.size === 0) return; // keep at least one on
		sections = next;
	}

	const items = $derived<MenuItem[]>(
		SECTIONS.map((sec) => ({
			label: sec.label,
			icon: sec.icon,
			pressed: sections.has(sec.value),
			keepOpen: true,
			onselect: () => toggle(sec.value),
			attrs: { 'data-journey': 'option', 'data-journey-key': sec.value }
		}))
	);
</script>

<div class="section-filter" data-journey="sections">
	<Menu
		label={m.sessions_filter_sections()}
		{items}
		placement="bottom-end"
		title={m.sessions_filter_sections_count({ count, total: SECTIONS.length })}
		style="--pop-box: var(--control-height)"
	>
		{#snippet trigger()}
			<Icon name="filter" size={18} />
		{/snippet}
	</Menu>
	{#if filtering}
		<span class="count" aria-hidden="true">
			<Badge size="xs" numeric tone="accent" style="--badge-bg: var(--accent); --badge-fg: var(--text-on-accent); --badge-border: var(--accent)">{count}</Badge>
		</span>
	{/if}
</div>

<style>
	.section-filter {
		position: relative;
		display: inline-flex;
		align-items: center;
		flex: none;
	}
	/* Shown when not all sections are enabled (a filter is active). `Menu` has no
	   trigger count prop, so the pill is positioned over the trigger here. */
	.count {
		position: absolute;
		top: -0.35rem;
		right: -0.35rem;
		pointer-events: none;
	}
</style>
