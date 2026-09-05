<script lang="ts">
	// The one route navigation: icon over label with the Sessions badge. At the
	// bottom it is the fixed phone bar; in the header it is the same items,
	// grouped closer and centred. Which one shows is the nav position setting.
	import { page } from '$app/state';
	import NavLink from '$lib/components/atoms/NavLink.svelte';
	import { useSessions } from '$lib/queries';
	import { settings } from '$lib/settings.svelte';
	import { isNavActive, navItems } from '$lib/navItems';
	import { m } from '$lib/paraglide/messages';

	let { placement = 'bottom' }: { placement?: 'bottom' | 'top' } = $props();

	// Top-level sessions carrying unread activity: the unit the list shows a
	// badge on. Children fold under their parent and are not counted twice.
	const sessions = useSessions(() => false);
	const unread = $derived(
		(sessions.data?.sessions ?? []).filter((s) => s.parent_id === null && (s.unread_count ?? 0) > 0)
			.length
	);
	const items = $derived(navItems());
</script>

<nav
	class="nav"
	class:bar={placement === 'bottom'}
	class:inline={placement === 'top'}
	class:hide-wide={placement === 'bottom' && settings.nav === 'top'}
	aria-label={m.nav_main_label()}
>
	<div class="nav-inner">
		{#each items as it (it.href)}
			{@const active = isNavActive(it.href, page.url.pathname)}
			<NavLink
				href={it.href}
				class="nav-btn {active ? 'active' : ''}"
				aria-current={active ? 'page' : undefined}
			>
				<span class="ico"
					>{it.icon}{#if it.href === '/sessions' && unread > 0}<span class="unread-badge"
							>{unread > 99 ? '99+' : unread}</span
						>{/if}</span
				>
				<span class="lbl">{it.label}</span>
			</NavLink>
		{/each}
	</div>
</nav>

<style>
	.nav.bar {
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
		.nav.hide-wide {
			display: none;
		}
	}
	.nav-inner {
		height: var(--nav-h);
		max-width: var(--content-wide);
		margin-inline: auto;
		display: flex;
	}
	.nav.inline {
		align-self: stretch;
		min-width: 0;
	}
	.nav.inline .nav-inner {
		--nav-btn-flex: none;
		--nav-btn-pad: var(--sp-3);
		height: 100%;
		max-width: none;
		justify-content: center;
		gap: var(--sp-1);
	}
	/* nav-btn is the class on the NavLink atom, so reach it via :global. */
	.nav-inner :global(.nav-btn) {
		flex: var(--nav-btn-flex, 1);
		padding-inline: var(--nav-btn-pad, 0);
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 2px;
		color: var(--text-faint);
		/* Fixed chrome (like the px-pinned header): deliberately not on the
		   font scale, so label and glyph never drift apart. */
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
