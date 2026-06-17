<!--
  The GitHub integration view (CCT-375 / GH-CAP-1).

  Lazy-loaded payload behind the `/github` route. Hosts two sections via tabs:
  the live PR inbox (GH-UI-1, default) and connector setup (GH-CONN-1). The
  inbox lists tracked PRs grouped by attention bucket and refreshes live off the
  ws `github_event` broadcast; connector setup stays one tab away so configuring
  GitHub accounts remains reachable.
-->
<script lang="ts">
	import { Heading, Stack, Tabs, type TabItem } from '@dorsk/tsumikit';
	import GithubInbox from './GithubInbox.svelte';
	import GithubConnectors from './GithubConnectors.svelte';
	import { useCapabilities } from '$lib/queries';

	const caps = useCapabilities();

	const tabs: TabItem[] = [
		{ id: 'inbox', label: 'Inbox' },
		{ id: 'connectors', label: 'Connectors' }
	];
	// First run (available but no connector yet) lands on Connectors so the
	// user can add their first GitHub account; once enabled, default to Inbox.
	let tab = $state($caps.data?.github.enabled === false ? 'connectors' : 'inbox');
</script>

<Stack gap="var(--sp-4)">
	<Heading level={1}>GitHub</Heading>
	<Tabs {tabs} bind:value={tab} label="GitHub sections">
		{#snippet panel(id)}
			{#if id === 'inbox'}
				<GithubInbox />
			{:else}
				<GithubConnectors />
			{/if}
		{/snippet}
	</Tabs>
</Stack>
