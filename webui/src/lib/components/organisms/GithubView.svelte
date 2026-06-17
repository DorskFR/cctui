<!--
  The GitHub integration view (CCT-375 / GH-CAP-1).

  Lazy-loaded payload behind the `/github` route. Hosts two sections via tabs:
  the live PR inbox (GH-UI-1, default) and connector setup (GH-CONN-1). The
  inbox lists tracked PRs grouped by attention bucket and refreshes live off the
  ws `github_event` broadcast; connector setup stays one tab away so configuring
  GitHub accounts remains reachable.

  It also hosts the shared "Review with agent" spawn modal (CCT-390 / GH-AGENT-1):
  the inbox rows and the diff viewer both emit a review-agent intent for a PR;
  here we resolve the repo-scoped review prompt (richelieu-style most-specific-
  wins) and open the EXISTING SpawnModal pre-seeded with the PR context, so the
  spawned session seeds its PR context and auto-links via SessionChild{kind:"pr"}.
-->
<script lang="ts">
	import { Heading, Stack, Tabs, type TabItem } from '@dorsk/tsumikit';
	import GithubInbox from './GithubInbox.svelte';
	import GithubConnectors from './GithubConnectors.svelte';
	import SpawnModal from './SpawnModal.svelte';
	import type { Form } from './spawn/types';
	import { useCapabilities, endpoints, type PullInboxItem } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';

	const caps = useCapabilities();

	const tabs: TabItem[] = [
		{ id: 'inbox', label: 'Inbox' },
		{ id: 'connectors', label: 'Connectors' }
	];
	// First run (available but no connector yet) lands on Connectors so the
	// user can add their first GitHub account; once enabled, default to Inbox.
	let tab = $state($caps.data?.github.enabled === false ? 'connectors' : 'inbox');

	// "Review with agent" spawn modal state (CCT-390). The prefill is built
	// asynchronously (it resolves the repo-scoped review prompt), so we keep a
	// "preparing" flag to avoid double-opens while the resolve is in flight.
	let spawnPrefill = $state<Partial<Form> | null>(null);
	let preparing = $state(false);

	// Build the PR context block that seeds the spawned session: a pointer the
	// agent can act on (it may `gh pr checkout` itself — that's its own tool use,
	// not a daemon data path, per doc §6.3 step 2).
	function prContext(pull: PullInboxItem): string {
		const url = `https://github.com/${pull.repo}/pull/${pull.number}`;
		return [
			`Review this pull request:`,
			`- repo: ${pull.repo}`,
			`- PR: #${pull.number} — ${pull.title}`,
			`- url: ${url}`,
			`- branch: ${pull.head_ref} → ${pull.base_ref}`
		].join('\n');
	}

	// Resolve the effective review prompt for the PR's repo, then open the spawn
	// modal pre-seeded. Most-specific-wins is decided server-side; a missing
	// prompt (no review prompt configured) just seeds the PR context alone.
	async function reviewWithAgent(pull: PullInboxItem) {
		if (preparing) return;
		preparing = true;
		try {
			const [owner, name] = pull.repo.split('/');
			let promptBody = prContext(pull);
			try {
				const resolved = await endpoints.resolveReviewPrompt(owner, name);
				if (resolved) promptBody = `${resolved.content}\n\n${promptBody}`;
			} catch {
				toasts.push('Could not load the review prompt; seeding PR context only', 'info');
			}
			spawnPrefill = {
				name: `review ${pull.repo}#${pull.number}`,
				prompt: promptBody,
				// Dispatch (k8s) PR context: the repo slug + an identity hint so a
				// dispatched worker can check the PR out itself. Harmless on a
				// machine spawn (those fields are dispatch-only).
				repo: pull.repo,
				identity: `review-pr-${pull.number}`
			};
		} finally {
			preparing = false;
		}
	}
</script>

<Stack gap="var(--sp-4)">
	<Heading level={1}>GitHub</Heading>
	<Tabs {tabs} bind:value={tab} label="GitHub sections">
		{#snippet panel(id)}
			{#if id === 'inbox'}
				<GithubInbox onreviewagent={reviewWithAgent} />
			{:else}
				<GithubConnectors />
			{/if}
		{/snippet}
	</Tabs>
</Stack>

{#if spawnPrefill}
	<SpawnModal
		prefill={spawnPrefill}
		onclose={() => (spawnPrefill = null)}
		onspawned={() => (spawnPrefill = null)}
	/>
{/if}
