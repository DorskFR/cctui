<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import SubagentBadge from '$lib/components/molecules/SubagentBadge.svelte';
	import { m } from '$lib/paraglide/messages';
	import type { SubagentToggle } from './view';

	// Fixed leading slot shared by the checkbox, the subagent toggles, the ↳
	// child marker and the star, so titles align across rows.
	let {
		session,
		child,
		selectable,
		selected,
		subagentToggles,
		onTogglePin
	}: {
		session: SessionListItem;
		child: boolean;
		selectable: boolean;
		selected: boolean;
		subagentToggles: SubagentToggle[];
		onTogglePin?: (s: SessionListItem) => void;
	} = $props();
</script>

{#if selectable}
	<span class="check" class:on={selected} aria-hidden="true">{selected ? '✓' : ''}</span>
{:else}
	<span class="gutter-group">
		{#each subagentToggles as t (t.key)}
			<SubagentBadge
				count={t.count}
				running={t.running}
				open={t.open}
				label={t.label}
				ontoggle={t.ontoggle}
			/>
		{/each}
		{#if child}
			<span class="indent" title={m.sessions_subagent_badge()} aria-hidden="true">↳</span>
		{:else if onTogglePin}
			<span
				class="star"
				class:on={session.pinned}
				role="button"
				tabindex="0"
				title={session.pinned ? m.sessions_unpin_title() : m.sessions_pin_title()}
				aria-pressed={session.pinned}
				aria-label={session.pinned ? m.sessions_unpin_aria() : m.sessions_pin_aria()}
				onpointerdown={(e) => e.stopPropagation()}
				onclick={(e) => {
					e.stopPropagation();
					onTogglePin?.(session);
				}}
				onkeydown={(e) => {
					if (e.key === 'Enter' || e.key === ' ') {
						e.preventDefault();
						e.stopPropagation();
						onTogglePin?.(session);
					}
				}}>{session.pinned ? '★' : '☆'}</span
			>
		{/if}
	</span>
{/if}

<style>
	.check {
		flex: none;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 1.15rem;
		height: 1.15rem;
		border-radius: var(--r-sm);
		border: 1.5px solid var(--border-strong);
		background: var(--bg);
		color: var(--bg);
		font-size: 0.8rem;
		line-height: 1;
	}
	.check.on {
		background: var(--accent);
		border-color: var(--accent);
		color: var(--bg);
	}
	.gutter-group {
		flex: none;
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
	}
	.indent,
	.star {
		flex: none;
		width: 14px;
		text-align: center;
		line-height: 1;
		font-size: var(--fs-md);
		color: var(--text-faint);
	}
	.star {
		background: none;
		border: none;
		cursor: pointer;
		user-select: none;
		padding: 0;
	}
	.star.on,
	.star:hover {
		color: var(--warn);
	}
</style>
