<!--
  Modal host for the virtualized diff viewer (GH-VIEW-3).

  Opened from a PR row in the inbox. Fetches the structured diff (GH-VIEW-1)
  via TanStack Query and hands it to `DiffViewer`. A live `GithubEvent` for this
  PR (a new push rotates the head SHA) invalidates the query so the diff
  refreshes — using the inbox's component-local `$state` reactivity pattern, not
  a keyed `$state` read off the ws singleton.

  A modal (not a deep route) is the lightest fit for this SPA: the app is a
  prerendered static fallback with file-based routing, and the inbox already
  owns the PR list — opening the diff in place keeps that context without a new
  dynamic route + loader.
-->
<script lang="ts">
	import type { PullInboxItem } from '$lib/queries';
	import { useGithubPullDiff } from '$lib/queries';
	import { ws } from '$lib/ws.svelte';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { Modal, Stack, Text } from '@dorsk/tsumikit';
	import DiffViewer from './DiffViewer.svelte';

	interface Props {
		pull: PullInboxItem;
		onclose: () => void;
	}
	const { pull, onclose }: Props = $props();

	const connectorId = $derived(pull.connector_id);
	const repo = $derived(pull.repo);
	const number = $derived(pull.number);

	const diff = useGithubPullDiff(
		() => connectorId,
		() => repo,
		() => number
	);

	const qc = useQueryClient();
	// Live nudge: a push to THIS PR rotates the head SHA → refetch the diff. We
	// keep a local tick so we never read the ws singleton's $state reactively.
	let liveTick = $state(0);
	$effect(() => {
		const unsub = ws.onGithubEvent((ev) => {
			if (ev.payload.repo === repo && ev.payload.pull_number === number) {
				liveTick++;
				qc.invalidateQueries({
					queryKey: ['github-pull-diff', connectorId, repo, number]
				});
			}
		});
		return unsub;
	});
</script>

<Modal
	title="{pull.repo}#{pull.number} · {pull.title}"
	size="lg"
	resizeKey="pull-diff-modal"
	{onclose}
>
	{#snippet body()}
		<Stack gap="var(--sp-2)">
			{#if $diff.isLoading}
				<Text tone="muted">Loading diff…</Text>
			{:else if $diff.isError}
				<Text tone="danger">Could not load the diff: {$diff.error?.message}</Text>
			{:else if $diff.data}
				{#if $diff.data.files.length === 0}
					<Text tone="muted">This pull request has no file changes.</Text>
				{:else}
					<DiffViewer diff={$diff.data} {connectorId} {number} />
				{/if}
			{/if}
		</Stack>
	{/snippet}
</Modal>
