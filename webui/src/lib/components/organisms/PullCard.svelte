<!--
  One PR row in the `/github` inbox (GH-UI-1), mirroring SessionCard's
  `Card as="div" tap` row layout. Purely presentational: it renders a synced PR
  plus its CI/review summary and emits an open intent — the inbox owns the data
  and live refresh.
-->
<script lang="ts">
	import type { PullInboxItem } from '$lib/queries';
	import { Badge, Button, Card, Cluster, Stack, Text, Timestamp } from '@dorsk/tsumikit';

	interface Props {
		pull: PullInboxItem;
		/** Open the PR on GitHub. */
		onopen?: (pull: PullInboxItem) => void;
		/** Open the virtualized diff viewer for this PR (GH-VIEW-3). */
		onreview?: (pull: PullInboxItem) => void;
	}
	const { pull, onopen, onreview }: Props = $props();

	const href = $derived(`https://github.com/${pull.repo}/pull/${pull.number}`);

	function open() {
		if (onopen) onopen(pull);
		else window.open(href, '_blank', 'noopener');
	}
	function review(e: MouseEvent) {
		e.stopPropagation();
		onreview?.(pull);
	}
	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' || e.key === ' ') {
			e.preventDefault();
			open();
		}
	}
</script>

<Card as="div" tap padding="sm" role="button" tabindex={0} onclick={open} {onkeydown}>
	<Stack gap="var(--sp-1)">
		<Cluster gap="var(--sp-2)" align="center" justify="space-between">
			<Cluster gap="var(--sp-2)" align="baseline">
				<Text weight="semibold" truncate>{pull.title}</Text>
				{#if pull.draft}
					<Badge tone="neutral">draft</Badge>
				{/if}
			</Cluster>
			<Cluster gap="var(--sp-2)" align="center">
				{#if onreview}
					<Button size="sm" onclick={review}>Review diff</Button>
				{/if}
				<Text tone="muted" size="xs">
					<Timestamp value={pull.gh_updated_at} mode="relative" />
				</Text>
			</Cluster>
		</Cluster>

		<Cluster gap="var(--sp-2)" align="center">
			<Text tone="muted" size="sm">{pull.repo}#{pull.number}</Text>
			<Text tone="muted" size="sm">{pull.author}</Text>
			{#if pull.reviews.changes_requested > 0}
				<Badge tone="danger">changes requested</Badge>
			{:else if pull.reviews.approved > 0}
				<Badge tone="ok">{pull.reviews.approved} approved</Badge>
			{/if}
			{#if pull.checks.failed > 0}
				<Badge tone="danger">{pull.checks.failed} CI failed</Badge>
			{:else if pull.checks.pending > 0}
				<Badge tone="warn">{pull.checks.pending} CI pending</Badge>
			{:else if pull.checks.passed > 0}
				<Badge tone="ok">CI green</Badge>
			{/if}
		</Cluster>
	</Stack>
</Card>
