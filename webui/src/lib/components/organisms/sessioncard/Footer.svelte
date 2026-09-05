<script lang="ts">
	import { m } from '$lib/paraglide/messages';
	import { safeHref } from '$lib/safeHref';
	import { Badge, Cluster, Icon, Stack, WorkingDir } from '@dorsk/tsumikit';
	import PrIcon from '$lib/components/atoms/PrIcon.svelte';
	import DraftActions from './DraftActions.svelte';
	import Readout from './Readout.svelte';
	import type { SessionActions, SessionView } from './view';

	// Three lines: cwd chip / branch chip + PR links / Σ ↑ ↓ ⚡ $ · model ·
	// effort · logo. The path and the branch keep their text; only the readout
	// degrades when the `sess-card` container is cramped.
	let { view, actions }: { view: SessionView; actions: SessionActions } = $props();

	const s = $derived(view.s);

	// A session can accumulate a dozen PRs; the footer is one line, so only the
	// most recent few are linked and the rest collapse into a hoverable count.
	const PR_SHOWN = 3;
	const shownPrs = $derived(view.prLinks.slice(0, PR_SHOWN));
	const hiddenPrs = $derived(Math.max(0, view.prLinks.length - PR_SHOWN));
	const allPrLabels = $derived(view.prLinks.map((p) => p.label).join('\n'));
</script>

<Stack gap="var(--sp-1)" style="min-width:0">
	<span class="cwd">
		<WorkingDir
			path={s.working_dir}
			copy
			full
			title={m.sessions_workdir_copy_title({ path: s.working_dir })}
			style="min-width:0;max-width:100%"
		/>
	</span>
	{#if view.branch || view.prLinks.length > 0}
		<Cluster wrap={false} gap="var(--sp-2)" align="center" style="min-width:0">
			{#if view.branch}
				<span class="branch" title={m.sessions_branch_title({ branch: view.branch })}>
					<Badge mono style="display:inline-flex;align-items:center;gap:0.25em;min-width:0;max-width:100%">
						<Icon name="fork" size={12} label={m.sessions_branch_label()} />
						<span class="branch-name">{view.branch}</span>
					</Badge>
				</span>
			{/if}
			{#if view.prLinks.length > 0}
				<span class="prs">
					{#each shownPrs as pr (pr.href)}
						<a
							class="pr-link"
							href={safeHref(pr.href)}
							target="_blank"
							rel="noopener noreferrer"
							title={m.sessions_pr_title({ label: pr.label })}
							onclick={(e) => e.stopPropagation()}
						>
							<PrIcon />
							<span class="pr-label">{pr.label}</span>
						</a>
					{/each}
					{#if hiddenPrs > 0}
						<span class="pr-more" title={allPrLabels}>+{hiddenPrs}</span>
					{/if}
				</span>
			{/if}
		</Cluster>
	{/if}
	<Cluster wrap={false} gap="var(--sp-2)" align="center" style="min-width:0;justify-content:flex-end">
		<Readout {view} />
		{#if view.draft}<DraftActions {view} {actions} />{/if}
	</Cluster>
</Stack>

<style>
	/* WorkingDir's fit algorithm reserves a rail-width share of the row, which
	   left a constant gap before the branch chip; `full` sizes it to the path
	   and this cap keeps a long one from pushing everything else out. */
	.cwd {
		display: inline-flex;
		min-width: 0;
		max-width: 100%;
	}
	.branch {
		display: inline-flex;
		min-width: 0;
		flex: 0 1 auto;
	}
	.branch-name {
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
	.prs {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
		flex: 0 1 auto;
		overflow: hidden;
	}
	.pr-link {
		display: inline-flex;
		align-items: center;
		gap: 0.25em;
		min-width: 0;
		font-size: var(--fs-xs);
		color: var(--accent);
		text-decoration: none;
		white-space: nowrap;
	}
	.pr-label {
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.pr-link:hover {
		text-decoration: underline;
	}
	.pr-more {
		flex: none;
		font-size: var(--fs-xs);
		color: var(--text-faint);
	}
</style>
