<script lang="ts">
	import { m } from '$lib/paraglide/messages';
	import { safeHref } from '$lib/safeHref';
	import { Cluster, Stack, Text } from '@dorsk/tsumikit';
	import PrIcon from '$lib/components/atoms/PrIcon.svelte';
	import DraftActions from './DraftActions.svelte';
	import Readout from './Readout.svelte';
	import { projectOf, type SessionActions, type SessionView } from './view';

	// Two lines: project · branch · PR links / tokens · cost · model · effort ·
	// logo. The full path is the title here and lives in the drawer.
	let { view, actions }: { view: SessionView; actions: SessionActions } = $props();

	const s = $derived(view.s);
	const where = $derived([projectOf(s.working_dir), view.branch].filter(Boolean).join(' · '));

	// A session can accumulate a dozen PRs; the footer is one line, so only the
	// most recent few are linked and the rest collapse into a hoverable count.
	const PR_SHOWN = 3;
	const shownPrs = $derived(view.prLinks.slice(0, PR_SHOWN));
	const hiddenPrs = $derived(Math.max(0, view.prLinks.length - PR_SHOWN));
	const allPrLabels = $derived(view.prLinks.map((p) => p.label).join('\n'));
</script>

<Stack gap="var(--sp-1)" style="min-width:0">
	<Cluster gap="var(--sp-2)" align="center" style="min-width:0">
		{#if where}
			<span class="where" title={view.branch ? `${s.working_dir} · ${view.branch}` : s.working_dir}>
				<Text size="xs" tone="muted" truncate>{where}</Text>
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
	<Cluster wrap={false} gap="var(--sp-2)" align="center" style="min-width:0">
		<Readout {view} spread />
		{#if view.draft}<DraftActions {view} {actions} />{/if}
	</Cluster>
</Stack>

<style>
	.where {
		display: inline-flex;
		min-width: 0;
		flex: 0 1 auto;
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
