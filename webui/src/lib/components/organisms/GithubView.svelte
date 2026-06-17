<!--
  The GitHub integration view (CCT-375 / GH-CAP-1).

  This is the lazy-loaded payload behind the `/github` route. It is imported via
  a dynamic `import()` so its bundle chunk — which later GH-* stories grow into a
  heavy diff viewer — is only fetched by users whose server reports the GitHub
  capability enabled. Non-GitHub users never download it.

  Placeholder for now: later stories fill in the PR list, diff viewer, and
  review composer. The deliverable of this story is the capability-gating +
  lazy-load plumbing, not the content.
-->
<script lang="ts">
	import { useCapabilities } from '$lib/queries';
	import { Card, Heading, Stack, Text } from '@dorsk/tsumikit';

	const caps = useCapabilities();
	const repos = $derived($caps.data?.github.repos ?? []);
</script>

<Stack gap="var(--sp-4)">
	<Heading level={1}>GitHub</Heading>
	<Card>
		<Stack gap="var(--sp-2)">
			<Text>
				GitHub integration is enabled. Pull-request review and diffs land in a
				later release.
			</Text>
			{#if repos.length > 0}
				<Text tone="faint">Tracked repos: {repos.join(', ')}</Text>
			{/if}
		</Stack>
	</Card>
</Stack>
