<script lang="ts">
	import type { Label } from '@bindings/Label';
	import { IconButton } from '@dorsk/tsumikit';
	import { clickOutside } from '$lib/clickOutside';
	import { labelTint } from '$lib/labels';

	// One square toolbar button that opens a popover of label toggles; a session
	// shows when it carries ANY selected label (OR semantics, CCT-360). `selected`
	// is bindable so the parent owns persistence. Renders nothing until at least
	// one label exists, so the caller doesn't have to guard.
	let {
		labels,
		selected = $bindable()
	}: { labels: Label[]; selected: Set<string> } = $props();

	let open = $state(false);

	function toggle(id: string) {
		const next = new Set(selected);
		if (next.has(id)) next.delete(id);
		else next.add(id);
		selected = next;
	}
</script>

{#if labels.length > 0}
	<div class="label-filter" use:clickOutside={() => (open = false)}>
		<IconButton
			class="btn-control-square"
			icon="tag"
			label="Filter by label"
			title={selected.size > 0 ? `Filtering by ${selected.size} label(s)` : 'Filter by label'}
			aria-haspopup="true"
			aria-expanded={open}
			aria-pressed={selected.size > 0}
			onclick={() => (open = !open)}
		/>
		{#if selected.size > 0}<span class="count-badge" aria-hidden="true">{selected.size}</span>{/if}
		{#if open}
			<div class="menu" role="menu" aria-label="Labels">
				{#each labels as l (l.id)}
					<button
						type="button"
						role="menuitemcheckbox"
						class="opt label-opt"
						style={labelTint(l)}
						aria-checked={selected.has(l.id)}
						onclick={() => toggle(l.id)}
					>
						<span class="check" aria-hidden="true">{selected.has(l.id) ? '✓' : ''}</span>
						<span class="opt-label">{l.name}</span>
					</button>
				{/each}
				{#if selected.size > 0}
					<button type="button" class="opt label-clear" onclick={() => (selected = new Set())}>
						<span class="check" aria-hidden="true">✕</span>
						<span class="opt-label">Clear filter</span>
					</button>
				{/if}
			</div>
		{/if}
	</div>
{/if}

<style>
	.label-filter {
		position: relative;
		display: inline-flex;
		align-items: center;
		flex: none;
	}
	.count-badge {
		position: absolute;
		top: -0.35rem;
		right: -0.35rem;
		min-width: 1rem;
		height: 1rem;
		padding: 0 0.25rem;
		border-radius: 999px;
		background: var(--accent);
		color: var(--bg);
		font-size: 0.62rem;
		font-weight: var(--fw-semibold);
		line-height: 1rem;
		text-align: center;
		pointer-events: none;
	}
	.menu {
		position: absolute;
		top: calc(100% + var(--sp-1));
		right: 0;
		z-index: 40;
		min-width: 12rem;
		max-height: 16rem;
		overflow-y: auto;
		display: flex;
		flex-direction: column;
		padding: var(--sp-1);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		background: var(--bg-elevated);
		box-shadow: var(--shadow-lg, 0 8px 24px rgba(0, 0, 0, 0.4));
	}
	.opt {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		width: 100%;
		padding: var(--sp-2);
		border: none;
		border-radius: var(--r-sm);
		background: none;
		color: var(--text);
		font-size: var(--fs-sm);
		text-align: left;
		cursor: pointer;
	}
	.opt:hover {
		background: var(--bg-hover, var(--border));
	}
	.check {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 1.05rem;
		height: 1.05rem;
		flex: none;
		border-radius: var(--r-sm);
		border: 1.5px solid var(--border-strong);
		font-size: 0.75rem;
		line-height: 1;
	}
	.opt-label {
		flex: 1 1 auto;
	}
	/* The whole label row carries the label's hue tint (set inline via labelTint),
	   so the menu reads at a glance by color. The inline background outranks the
	   generic .opt:hover, so the tint persists on hover; a faint inset ring marks
	   the hovered/checked row. */
	.label-opt {
		margin-bottom: 2px;
		border: 1px solid transparent;
	}
	.label-opt:last-child {
		margin-bottom: 0;
	}
	.label-opt:hover {
		box-shadow: inset 0 0 0 1px var(--text);
	}
	.label-opt .opt-label {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	/* The checkbox needs a SOLID surface of its own — a transparent box would let
	   the row tint show through and effectively vanish. A neutral fill + strong
	   border reads as a distinct control on any hue; the checked state is the
	   familiar accent fill + ✓. */
	.label-opt .check {
		background: var(--bg-elevated);
		border-color: var(--border-strong);
		color: var(--text);
	}
	.label-opt[aria-checked='true'] .check {
		background: var(--accent);
		border-color: var(--accent);
		color: var(--bg);
	}
	.label-clear {
		color: var(--text-muted);
	}
</style>
