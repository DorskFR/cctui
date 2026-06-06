<script lang="ts">
	import type { SessionListItem } from '$lib/bindings/SessionListItem';
	import SessionCard from './SessionCard.svelte';

	// A collapsible group of Workflow-tool subagents (CCT-225) that share one
	// `workflow_run_id`. A single deep-research run can spawn 100+ agents, so
	// they collapse by default and the header shows running/done counts.
	let {
		runId,
		name,
		agents,
		compact = false,
		pending,
		onopen,
		selecting = false,
		selected,
		onToggleSelect,
		swipeArchive
	}: {
		runId: string;
		name: string | null;
		agents: SessionListItem[];
		compact?: boolean;
		pending: (id: string) => number;
		onopen: (s: SessionListItem) => void;
		selecting?: boolean;
		selected: Set<string>;
		onToggleSelect: (s: SessionListItem) => void;
		swipeArchive: (s: SessionListItem) => void;
	} = $props();

	let open = $state(false);

	// "running" = anything still live/working; "done" = ended/completed.
	const running = $derived(
		agents.filter((a) => a.status !== 'archived' && a.liveness !== 'dead' && !a.hibernated).length
	);
	const done = $derived(agents.length - running);
	const label = $derived(name ? `Workflow: ${name}` : 'Workflow');
</script>

<button class="wf-header" class:compact onclick={() => (open = !open)} type="button">
	<span class="caret" class:open>▸</span>
	<span class="wf-name">{label}</span>
	<span class="wf-run">{runId}</span>
	<span class="wf-counts">
		<span class="count">{agents.length}</span>
		{#if running > 0}<span class="run">{running} running</span>{/if}
		{#if done > 0}<span class="done">{done} done</span>{/if}
	</span>
</button>
{#if open}
	{#each agents as a (a.id)}
		<SessionCard
			session={a}
			child
			{compact}
			pendingCount={pending(a.id)}
			onopen={(x) => onopen(x)}
			selectable={selecting}
			selected={selected.has(a.id)}
			onToggleSelect={onToggleSelect}
			swipeable
			swipeLabel="Archive"
			onSwipe={swipeArchive}
		/>
	{/each}
{/if}

<style>
	.wf-header {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		width: 100%;
		padding: 0.4rem 0.6rem 0.4rem 1.6rem;
		background: var(--surface-2, rgba(255, 255, 255, 0.03));
		border: none;
		border-left: 2px solid var(--border, rgba(255, 255, 255, 0.12));
		color: inherit;
		font: inherit;
		text-align: left;
		cursor: pointer;
	}
	.wf-header.compact {
		padding-left: 0.8rem;
	}
	.wf-header:hover {
		background: var(--surface-3, rgba(255, 255, 255, 0.06));
	}
	.caret {
		display: inline-block;
		transition: transform 0.1s ease;
		opacity: 0.7;
		font-size: 0.75em;
	}
	.caret.open {
		transform: rotate(90deg);
	}
	.wf-name {
		font-weight: 600;
	}
	.wf-run {
		opacity: 0.6;
		font-family: var(--font-mono, monospace);
		font-size: 0.85em;
	}
	.wf-counts {
		margin-left: auto;
		display: flex;
		align-items: center;
		gap: 0.4rem;
		font-size: 0.8em;
	}
	.count {
		opacity: 0.7;
	}
	.run {
		color: var(--accent, #4ea1ff);
	}
	.done {
		opacity: 0.6;
	}
</style>
