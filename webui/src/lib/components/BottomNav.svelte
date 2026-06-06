<script lang="ts">
	import { page } from '$app/state';

	const items = [
		{ href: '/', label: 'Overview', icon: '◧' },
		{ href: '/sessions', label: 'Sessions', icon: '◰' },
		{ href: '/users', label: 'Users', icon: '◍' },
		{ href: '/dispatchers', label: 'Dispatchers', icon: '◈' }
	];
	const active = (href: string) =>
		href === '/' ? page.url.pathname === '/' : page.url.pathname.startsWith(href);
</script>

<nav class="nav">
	<div class="nav-inner">
		{#each items as it (it.href)}
			<a class="nav-btn" class:active={active(it.href)} href={it.href}>
				<span class="ico">{it.icon}</span>
				<span class="lbl">{it.label}</span>
			</a>
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
	.nav-btn {
		flex: 1;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 2px;
		color: var(--text-faint);
		font-size: var(--fs-xs);
		text-decoration: none;
		font-weight: var(--fw-medium);
	}
	.nav-btn .ico {
		font-size: 1.25rem;
		line-height: 1;
	}
	.nav-btn.active {
		color: var(--accent);
	}
	.nav-btn:active {
		background: var(--bg-elevated-2);
	}
</style>
