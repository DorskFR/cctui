<script lang="ts">
	import { page } from '$app/state';
	import { Badge } from '@dorsk/tsumikit';
	import NavLink from '$lib/components/atoms/NavLink.svelte';
	import { useSessions } from '$lib/queries';
	import { isNavActive, navItems } from '$lib/navItems';
	import { m } from '$lib/paraglide/messages';

	const sessions = useSessions(() => false);
	// Sessions carrying unread activity, not messages summed across them: the
	// tab counts the same unit it names, and cannot run away to "99+".
	const unreadSessions = $derived(
		(sessions.data?.sessions ?? []).filter((s) => (s.unread_count ?? 0) > 0).length
	);
	const items = $derived(navItems());
</script>

<nav class="tabs" aria-label={m.nav_main_label()}>
	{#each items as it (it.href)}
		<NavLink href={it.href} variant="tab" active={isNavActive(it.href, page.url.pathname)}>
			<span class="tab-label">{it.label}</span>
			{#if it.href === '/sessions' && unreadSessions > 0}
				<Badge tone="danger" size="xs">{unreadSessions}</Badge>
			{/if}
		</NavLink>
	{/each}
</nav>

<style>
	.tabs {
		display: flex;
		align-items: stretch;
		align-self: stretch;
		min-width: 0;
		overflow: hidden;
		font-size: var(--fs-sm);
		font-weight: var(--fw-medium);
	}
	.tab-label {
		white-space: nowrap;
	}
</style>
