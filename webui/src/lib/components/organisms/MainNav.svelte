<script lang="ts">
	// The one route navigation: icon over label with the Sessions badge. At the
	// bottom it is the fixed phone bar; in the header it is the same items laid
	// out the same way. Which one shows is the nav position setting.
	import { page } from '$app/state';
	import { Badge, Icon } from '@dorsk/tsumikit';
	import NavLink from '$lib/components/atoms/NavLink.svelte';
	import { usePlugins, useSessions } from '$lib/queries';
	import { pageNavItems } from '$lib/plugins/pageRoute';
	import { settings } from '$lib/settings.svelte';
	import { isNavActive, navItems, navKey } from '$lib/navItems';
	import { m } from '$lib/paraglide/messages';

	let { placement = 'bottom' }: { placement?: 'bottom' | 'top' } = $props();

	// Top-level sessions carrying unread activity: the unit the list shows a
	// badge on. Children fold under their parent and are not counted twice.
	const sessions = useSessions(() => false);
	const unread = $derived(
		(sessions.data?.sessions ?? []).filter((s) => s.parent_id === null && (s.unread_count ?? 0) > 0)
			.length
	);
	const plugins = usePlugins();
	const items = $derived(
		navItems({
			pages: pageNavItems(plugins.data ?? [], settings.pluginsEnabled)
		})
	);
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
				aria-current={active ? 'page' : undefined}
				data-journey="nav"
				data-journey-key={navKey(it.href)}
			>
				<span class="cell" class:active>
					<span class="ico"
						><Icon name={it.iconName} size={20} />{#if it.href === '/sessions' && unread > 0}<span class="unread"
								><Badge
									size="xs"
									numeric
									tone="danger"
									style="--badge-bg: var(--danger); --badge-fg: var(--text-on-accent); --badge-border: var(--danger)"
									>{unread > 99 ? '99+' : unread}</Badge
								></span
							>{/if}</span
					>
					<span class="lbl">{it.label}</span>
				</span>
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
	/* Equal columns instead of `flex: 1` on each anchor: the grid sizes the
	   NavLink roots from the outside, so no rule has to reach into the atom. */
	.nav-inner {
		height: var(--nav-h);
		max-width: var(--content-wide);
		margin-inline: auto;
		display: grid;
		grid-auto-flow: column;
		grid-auto-columns: 1fr;
		min-width: 0;
	}
	.nav.inline {
		align-self: stretch;
		width: 100%;
		min-width: 0;
	}
	/* Only the height differs: the header owns it. Everything else is the bar's. */
	.nav.inline .nav-inner {
		height: 100%;
	}
	.lbl {
		max-width: 100%;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	/* In the header the nav takes what the brand and the account cluster leave,
	   so a label gives way inside its own cell rather than running over whatever
	   sits beside it. */
	.cell {
		display: flex;
		width: 100%;
		height: 100%;
		min-width: 0;
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
	.unread {
		position: absolute;
		top: -0.4rem;
		left: 60%;
		pointer-events: none;
	}
	.cell.active {
		color: var(--accent);
	}
	.cell:active {
		background: var(--bg-elevated-2);
	}
</style>
