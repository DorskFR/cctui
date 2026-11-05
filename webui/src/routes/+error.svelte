<script lang="ts">
	import { page } from '$app/state';
	import { Button, Stack, Text } from '@dorsk/tsumikit';
	import PageHead from '$lib/components/molecules/PageHead.svelte';
	import { useSessions } from '$lib/queries';
	import { displayNameOf } from '$lib/sessionName';
	import { m } from '$lib/paraglide/messages';
	import { isNotFound, recentSessions } from './error.logic';

	const notFound = $derived(isNotFound(page.status));
	const sessions = useSessions(() => false);
	const recent = $derived(recentSessions(sessions.data?.sessions));
</script>

<Stack gap="var(--sp-6)">
	<PageHead
		title={notFound ? m.shell_error_not_found_title() : m.shell_error_title()}
		subtitle={notFound
			? m.shell_error_not_found_body()
			: (page.error?.message ?? m.common_error())}
	/>

	{#if notFound}
		<Text tone="muted" size="sm">
			<span class="path">{page.url.pathname}</span>
		</Text>
	{/if}

	<div class="actions">
		<Button variant="primary" href="/sessions">{m.shell_error_go_sessions()}</Button>
		{#if !notFound}
			<Button onclick={() => location.reload()}>{m.common_retry()}</Button>
		{/if}
	</div>

	{#if recent.length > 0}
		<section class="recent">
			<Text variant="eyebrow" tone="muted">{m.shell_error_recent()}</Text>
			<ul>
				{#each recent as s (s.id)}
					<li><a href={`/sessions/${s.id}`}>{displayNameOf(s)}</a></li>
				{/each}
			</ul>
		</section>
	{/if}
</Stack>

<style>
	.path {
		font-family: var(--font-mono);
		overflow-wrap: anywhere;
	}
	.actions {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-2);
	}
	.recent {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	ul {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		margin: 0;
		padding: 0;
		list-style: none;
	}
	a {
		display: inline-flex;
		align-items: center;
		min-height: var(--hit-min, 24px);
		color: var(--text);
		text-decoration: none;
	}
	a:hover {
		text-decoration: underline;
	}
</style>
