<!--
  The `/github` PR inbox (GH-UI-1): a live list of tracked PRs grouped by
  attention bucket, with a text filter, reusing PullCard rows.

  Live updates: subscribes to the ws `github_event` broadcast (GH-CONN-5) via the
  `onGithubEvent` callback and invalidates the `github-pulls` query so TanStack
  refetches. Per the webui reactivity rule we keep a component-local `$state`
  tick and do NOT read a keyed `$state` off the ws singleton through `$derived`.
-->
<script lang="ts">
	import { useGithubPulls, type PullInboxItem } from '$lib/queries';
	import { ws } from '$lib/ws.svelte';
	import { useQueryClient } from '@tanstack/svelte-query';
	import type { AttentionBucket } from '@bindings/AttentionBucket';
	import { Card, Heading, Stack, Text } from '@dorsk/tsumikit';
	import SearchBox from '../molecules/SearchBox.svelte';
	import PullCard from './PullCard.svelte';

	const pulls = useGithubPulls();
	const qc = useQueryClient();

	// Live nudge → refetch. A small local tick proves the subscription is wired
	// without reading the ws singleton's internal state reactively.
	let liveTick = $state(0);
	$effect(() => {
		const unsub = ws.onGithubEvent(() => {
			liveTick++;
			qc.invalidateQueries({ queryKey: ['github-pulls'] });
		});
		return unsub;
	});

	let query = $state('');

	// Bucket render order + headers, mirroring the AttentionBucket priority.
	const BUCKETS: { key: AttentionBucket; label: string }[] = [
		{ key: 'needs_my_review', label: 'Needs my review' },
		{ key: 'my_pr_changes_requested', label: 'My PR — changes requested' },
		{ key: 'my_pr_ci_red', label: 'My PR — CI red' },
		{ key: 'my_pr_mergeable', label: 'My PR — mergeable' },
		{ key: 'waiting', label: 'Waiting' }
	];

	const all = $derived($pulls.data ?? []);
	const matches = (p: PullInboxItem) => {
		const q = query.trim().toLowerCase();
		if (!q) return true;
		return (
			p.title.toLowerCase().includes(q) ||
			p.repo.toLowerCase().includes(q) ||
			p.author.toLowerCase().includes(q) ||
			String(p.number).includes(q)
		);
	};
	const groups = $derived(
		BUCKETS.map((b) => ({
			...b,
			pulls: all.filter((p) => p.bucket === b.key && matches(p))
		})).filter((g) => g.pulls.length > 0)
	);
</script>

<Stack gap="var(--sp-4)">
	<SearchBox bind:value={query} placeholder="Filter PRs by title, repo, author…" />

	{#if $pulls.isLoading}
		<Card><Text tone="muted">Loading pull requests…</Text></Card>
	{:else if all.length === 0}
		<Card>
			<Text tone="muted">
				No tracked pull requests yet. Add a connector and the reconcile poll will hydrate your
				inbox.
			</Text>
		</Card>
	{:else if groups.length === 0}
		<Card><Text tone="muted">No pull requests match “{query}”.</Text></Card>
	{:else}
		{#each groups as g (g.key)}
			<Stack gap="var(--sp-2)">
				<Heading level={3}>{g.label} ({g.pulls.length})</Heading>
				<Stack gap="var(--sp-2)">
					{#each g.pulls as p (`${p.connector_id}:${p.repo}:${p.number}`)}
						<PullCard pull={p} />
					{/each}
				</Stack>
			</Stack>
		{/each}
	{/if}
</Stack>
