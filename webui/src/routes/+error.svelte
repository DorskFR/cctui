<script lang="ts">
	import { page } from '$app/state';
	import { Button, Heading, Link, Text } from '@dorsk/tsumikit';
	import { useSessions } from '#lib/queries/index.ts';
	import { displayNameOf } from '#lib/sessionName.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { errorCopy, recentSessions } from './error.logic.ts';

	const missingSession = $derived(page.error?.message === 'session_missing');
	const copy = $derived(errorCopy(page.status, missingSession, page.url.pathname));

	const live = useSessions(() => false);
	const recent = $derived(recentSessions(live.data?.sessions ?? [], 5));
	const nameOf = (s: (typeof recent)[number]) => displayNameOf(s) || s.working_dir || s.id;
</script>

<div class="err">
	<div class="mark" aria-hidden="true">
		<span class="prompt">»_</span><span class="code">{page.status}</span>
	</div>
	<Heading level={1} size="xl">{copy.title}</Heading>
	<Text tone="muted">{copy.body}</Text>
	{#if !missingSession}
		<Button variant="primary" href="/sessions">{m.error_go_to_sessions()}</Button>
	{/if}
	{#if recent.length > 0}
		<div class="recent">
			<Text weight="semibold">{m.error_recent_sessions()}</Text>
			<ul class="links">
				{#each recent as s (s.id)}
					<li><Link href={`/sessions/${s.id}`}>{nameOf(s)}</Link></li>
				{/each}
			</ul>
		</div>
	{/if}
</div>

<style>
	.err {
		min-height: calc(
			100dvh - var(--header-h) - var(--safe-top) - var(--bottom-chrome) - var(--sp-3) - var(--sp-4)
		);
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: var(--sp-4);
		text-align: center;
		padding-bottom: var(--sp-10);
	}
	.mark {
		display: flex;
		align-items: baseline;
		gap: var(--sp-3);
		font-weight: var(--fw-bold);
		line-height: 1;
	}
	.prompt {
		font-family: var(--font-mono);
		font-size: 3rem;
		color: var(--accent);
	}
	.code {
		font-size: 6rem;
		font-variant-numeric: tabular-nums;
		letter-spacing: -0.02em;
		color: var(--text-faint);
	}
	.recent {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: var(--sp-2);
		margin-top: var(--sp-4);
	}
	.links {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: var(--sp-2);
	}
</style>
