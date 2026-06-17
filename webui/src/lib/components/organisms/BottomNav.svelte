<script lang="ts">
	import { page } from '$app/state';
	import NavLink from '$lib/components/atoms/NavLink.svelte';
	import { useCapabilities } from '$lib/queries';

	const caps = useCapabilities();

	// The `/github` item is mounted when the integration is *available* (crate
	// compiled in + schema present), NOT when it's enabled — so the connector
	// setup UI is reachable to add the first connector (CCT-395). Hidden when
	// unavailable → the lazy route chunk is never reachable for non-GitHub users.
	const items = $derived([
		{ href: '/', label: 'Overview', icon: '◧' },
		{ href: '/sessions', label: 'Sessions', icon: '◰' },
		{ href: '/users', label: 'Users', icon: '◍' },
		{ href: '/dispatchers', label: 'Dispatchers', icon: '◈' },
		{ href: '/accounts', label: 'Accounts', icon: '◉' },
		...($caps.data?.github.available ? [{ href: '/github', label: 'GitHub', icon: '◐' }] : []),
		{ href: '/settings', label: 'Settings', icon: '⚙' }
	]);
	const active = (href: string) =>
		href === '/' ? page.url.pathname === '/' : page.url.pathname.startsWith(href);
</script>

<nav class="nav">
	<div class="nav-inner">
		{#each items as it (it.href)}
			<NavLink href={it.href} class="nav-btn {active(it.href) ? 'active' : ''}">
				<span class="ico">{it.icon}</span>
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
	}
	:global(.nav-btn.active) {
		color: var(--accent);
	}
	:global(.nav-btn:active) {
		background: var(--bg-elevated-2);
	}
</style>
