<script lang="ts">
	import { page } from '$app/state';
	import NavLink from '$lib/components/atoms/NavLink.svelte';
	import { useSessions } from '$lib/queries';
	import { settings } from '$lib/settings.svelte';
	import { isNavActive, navItems } from '$lib/navItems';
	import { m } from '$lib/paraglide/messages';

	// Aggregate unread count across the live list, surfaced as a red
	// pill on the Sessions item. The list is already fetched app-wide (Header),
	// so this shares the query cache — no extra request.
	const sessions = useSessions(() => false);
	const totalUnread = $derived(
		(sessions.data?.sessions ?? []).reduce((n, s) => n + (s.unread_count ?? 0), 0)
	);

	const items = $derived(navItems());
</script>

<nav class="nav" class:top={settings.nav === 'top'} aria-label={m.nav_main_label()}>
	<div class="nav-inner">
		{#each items as it (it.href)}
			{@const active = isNavActive(it.href, page.url.pathname)}
			<NavLink
				href={it.href}
				class="nav-btn {active ? 'active' : ''}"
				aria-current={active ? 'page' : undefined}
			>
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
	@media (min-width: 48rem) {
		.nav.top {
			display: none;
		}
	}
	.nav-inner {
		height: var(--nav-h);
		max-width: var(--content-wide);
		margin-inline: auto;
		display: flex;
	}
	/* nav-btn is the class on the NavLink atom, so reach it via :global. */
	.nav-inner :global(.nav-btn) {
		flex: 1;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 2px;
		color: var(--text-faint);
		/* Footer is fixed chrome (like the px-pinned header) — it deliberately does
		   NOT respond to the font-scale picker, so use fixed sizes for both the
		   label and the glyph so they scale together / not at all. */
		font-size: 0.6875rem;
		font-weight: var(--fw-medium);
	}
	.nav-inner .ico {
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
		color: var(--text-on-accent);
		font-size: 0.62rem;
		font-weight: var(--fw-semibold);
		line-height: 1rem;
		text-align: center;
		pointer-events: none;
	}
	.nav-inner :global(.nav-btn.active) {
		color: var(--accent);
	}
	.nav-inner :global(.nav-btn:active) {
		background: var(--bg-elevated-2);
	}
</style>
