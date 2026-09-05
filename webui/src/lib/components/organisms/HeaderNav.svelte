<script lang="ts">
	import { page } from '$app/state';
	import { NavItem } from '@dorsk/tsumikit';
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
		<NavItem
			href={it.href}
			label={it.label}
			active={isNavActive(it.href, page.url.pathname)}
			activeStyle="bar"
			badge={it.href === '/sessions' && unreadSessions > 0 ? unreadSessions : undefined}
			badgeTone="danger"
		/>
	{/each}
</nav>

<style>
	.tabs {
		display: flex;
		align-items: center;
		gap: var(--sp-1);
		min-width: 0;
		overflow: hidden;
	}
</style>
