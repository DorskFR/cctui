<script lang="ts">
	import { page } from '$app/state';
	import { NavItem } from '@dorsk/tsumikit';
	import { useSessions } from '$lib/queries';
	import { isNavActive, navItems } from '$lib/navItems';
	import { m } from '$lib/paraglide/messages';

	const sessions = useSessions(() => false);
	const totalUnread = $derived(
		(sessions.data?.sessions ?? []).reduce((n, s) => n + (s.unread_count ?? 0), 0)
	);
	const items = $derived(navItems());
</script>

<nav class="tabs" aria-label={m.nav_main_label()}>
	{#each items as it (it.href)}
		<span class="tab">
			<NavItem
				href={it.href}
				label={it.label}
				active={isNavActive(it.href, page.url.pathname)}
				badge={it.href === '/sessions' && totalUnread > 0
					? totalUnread > 99
						? '99+'
						: totalUnread
					: undefined}
				badgeTone="danger"
			/>
		</span>
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
	.tab {
		display: inline-flex;
		flex: none;
		font-size: var(--fs-sm);
		font-weight: var(--fw-medium);
	}
</style>
