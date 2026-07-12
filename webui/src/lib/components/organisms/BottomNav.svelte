<script lang="ts">
	import { page } from '$app/state';
	import NavLink from '$lib/components/atoms/NavLink.svelte';
	import { useCapabilities, useSessions } from '$lib/queries';
	import { ghreviewUrl } from '$lib/config';
	import { m } from '$lib/paraglide/messages';

	const caps = useCapabilities();

	// Aggregate unread count (CCT-580) across the live list, surfaced as a red
	// pill on the Sessions item. The list is already fetched app-wide (Header),
	// so this shares the query cache — no extra request.
	const sessions = useSessions(() => false);
	const totalUnread = $derived(
		($sessions.data?.sessions ?? []).reduce((n, s) => n + (s.unread_count ?? 0), 0)
	);

	// gh-review connector (CCT-610) is gated on a client-side deploy config value
	// (`ghreviewUrl`), not a server capability like GitHub below.
	const reviewEnabled = ghreviewUrl() !== null;

	// The `/github` item is mounted only when the integration is *enabled* (a
	// connector is configured), NOT merely available (CCT-403) — first-run
	// connector setup now lives under Accounts, so there's no chicken-and-egg.
	// This frees nav space for the many users who don't use GitHub. Dispatchers
	// also moved under Accounts, which is now the single home for everything that
	// connects to something external.
	const items = $derived([
		{ href: '/', label: m.nav_overview(), icon: '◧' },
		{ href: '/sessions', label: m.nav_sessions(), icon: '◰' },
		{ href: '/users', label: m.nav_users(), icon: '◍' },
		{ href: '/accounts', label: m.nav_accounts(), icon: '◉' },
		...($caps.data?.github.enabled ? [{ href: '/github', label: m.nav_github(), icon: '◐' }] : []),
		...(reviewEnabled ? [{ href: '/review', label: m.nav_review(), icon: '◫' }] : []),
		{ href: '/settings', label: m.nav_settings(), icon: '⚙' }
	]);
	const active = (href: string) =>
		href === '/' ? page.url.pathname === '/' : page.url.pathname.startsWith(href);
</script>

<nav class="nav">
	<div class="nav-inner">
		{#each items as it (it.href)}
			<NavLink href={it.href} class="nav-btn {active(it.href) ? 'active' : ''}">
				<span class="ico"
					>{it.icon}{#if it.href === '/sessions' && totalUnread > 0}<span
							class="unread-badge">{totalUnread > 99 ? '99+' : totalUnread}</span
						>{/if}</span
				>
				<span class="lbl">{it.label}</span>
			</NavLink>
		{/each}
	</div>
</nav>

<style>
	.nav {
		position: fixed;
		bottom: 0;
		left: 0;
		right: 0;
		z-index: var(--z-nav);
		background: color-mix(in srgb, var(--bg-elevated) 95%, transparent);
		backdrop-filter: blur(8px);
		border-top: 1px solid var(--border);
		padding-bottom: var(--safe-bottom);
	}
	.nav-inner {
		height: var(--nav-h);
		max-width: var(--content-max);
		margin-inline: auto;
		display: flex;
	}
	/* nav-btn is the class on the NavLink atom, so reach it via :global. */
	:global(.nav-btn) {
		flex: 1;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 2px;
		color: var(--text-faint);
		/* Footer is fixed chrome (like the px-pinned header) — it deliberately does
		   NOT respond to the font-scale picker, so use fixed sizes for both the
		   label and the glyph so they scale together / not at all (CCT-345). */
		font-size: 0.6875rem;
		font-weight: var(--fw-medium);
	}
	:global(.nav-btn) .ico {
		font-size: 1.25rem;
		line-height: 1;
		position: relative;
	}
	.unread-badge {
		position: absolute;
		top: -0.4rem;
		left: 60%;
		min-width: 1rem;
		height: 1rem;
		padding: 0 0.22rem;
		border-radius: 999px;
		background: var(--danger);
		color: var(--text-on-accent, #fff);
		font-size: 0.62rem;
		font-weight: var(--fw-semibold);
		line-height: 1rem;
		text-align: center;
		pointer-events: none;
	}
	:global(.nav-btn.active) {
		color: var(--accent);
	}
	:global(.nav-btn:active) {
		background: var(--bg-elevated-2);
	}
</style>
