<script lang="ts">
	import { m } from '$lib/paraglide/messages';
	import { safeHref } from '$lib/safeHref';
	import { Badge, Cluster, Icon, WorkingDir } from '@dorsk/tsumikit';
	import DraftActions from './DraftActions.svelte';
	import Readout from './Readout.svelte';
	import type { SessionActions, SessionView } from './view';

	// cwd chip · branch chip / PR link ···· Σ ↑ ↓ ⚡ $ · model · effort · logo.
	// The branch collapses to its ⑂ glyph (tooltip keeps the name) when the
	// `sess-card` container is cramped.
	let { view, actions }: { view: SessionView; actions: SessionActions } = $props();

	const s = $derived(view.s);
</script>

<Cluster wrap={false} gap="var(--sp-2)" align="center" style="min-width:0">
	<WorkingDir
		path={s.working_dir}
		copy
		title={m.sessions_workdir_copy_title({ path: s.working_dir })}
		style="flex:0 1 auto;min-width:0"
	/>
	{#if view.branch}
		<span class="branch" title={m.sessions_branch_title({ branch: view.branch })}>
			<Badge mono style="display:inline-flex;align-items:center;gap:0.25em;min-width:0;max-width:100%">
				<Icon name="fork" size={12} label={m.sessions_branch_label()} />
				<span class="branch-name">{view.branch}</span>
			</Badge>
		</span>
	{/if}
	{#each view.prLinks as pr (pr.href)}
		<a
			class="pr-link"
			href={safeHref(pr.href)}
			target="_blank"
			rel="noopener noreferrer"
			title={m.sessions_pr_title({ label: pr.label })}
			onclick={(e) => e.stopPropagation()}>⇄ {pr.label}</a
		>
	{/each}
	<Cluster wrap={false} gap="var(--sp-2)" style="margin-left:auto;flex:none">
		<Readout {view} />
		{#if view.draft}<DraftActions {view} {actions} />{/if}
	</Cluster>
</Cluster>

<style>
	.branch {
		display: inline-flex;
		min-width: 0;
		max-width: 45%;
		flex: 0 1 auto;
	}
	.branch-name {
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
	.pr-link {
		flex: none;
		font-size: var(--fs-xs);
		color: var(--accent);
		text-decoration: none;
		white-space: nowrap;
	}
	.pr-link:hover {
		text-decoration: underline;
	}
	@container sess-card (max-width: 16rem) {
		.branch-name {
			display: none;
		}
	}
</style>
