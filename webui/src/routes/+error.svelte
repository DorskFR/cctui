<script lang="ts">
	import { page } from '$app/state';
	import { Button, Input, Link, Stack, Text } from '@dorsk/tsumikit';
	import PageHead from '#lib/components/molecules/PageHead.svelte';
	import { useSessions } from '#lib/queries/index.ts';
	import { displayNameOf } from '#lib/sessionName.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { errorCopy, matchSessions, recentSessions } from './error.logic.ts';

	const missingSession = $derived(page.error?.message === 'session_missing');
	const copy = $derived(errorCopy(page.status, missingSession, page.url.pathname));

	const live = useSessions(() => false);
	const archived = useSessions(
		() => true,
		() => missingSession
	);
	let query = $state('');

	const recent = $derived(recentSessions(live.data?.sessions ?? [], 5));
	const matches = $derived(
		matchSessions(archived.data?.sessions ?? [], query, displayNameOf).slice(0, 8)
	);
	const nameOf = (s: (typeof recent)[number]) => displayNameOf(s) || s.working_dir || s.id;
</script>

{#snippet sessionLinks(list: typeof recent)}
	<ul class="links">
		{#each list as s (s.id)}
			<li><Link href={`/sessions/${s.id}`}>{nameOf(s)}</Link></li>
		{/each}
	</ul>
{/snippet}

<Stack gap="var(--sp-5)">
	<PageHead title={copy.title} />
	<Text tone="muted">{copy.body}</Text>
	<div>
		<Button variant="primary" href="/sessions">{m.error_go_to_sessions()}</Button>
	</div>

	{#if missingSession}
		<Stack gap="var(--sp-2)">
			<Text weight="semibold">{m.error_search_archived()}</Text>
			<Input
				bind:value={query}
				icon="search"
				clearable
				aria-label={m.error_search_archived()}
				placeholder={m.error_search_archived_placeholder()}
			/>
			{#if query.trim() && matches.length === 0}
				<Text size="sm" tone="faint">{m.error_search_archived_none()}</Text>
			{:else if matches.length > 0}
				{@render sessionLinks(matches)}
			{/if}
		</Stack>
	{:else if recent.length > 0}
		<Stack gap="var(--sp-2)">
			<Text weight="semibold">{m.error_recent_sessions()}</Text>
			{@render sessionLinks(recent)}
		</Stack>
	{/if}
</Stack>

<style>
	.links {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
</style>
