<script lang="ts">
	// Navigational/chrome anchor primitive: the only place a non-inline <a> is
	// emitted (the inline underlined text link is the separate Link atom). Owns a
	// minimal reset (no underline, inherits colour); consumers style the specific
	// look (bottom-nav item, version chip) via a passed class, or pick the `tab`
	// variant for the full-height header tab. Passes through href/target/rel and
	// every native attribute.
	import type { Snippet } from 'svelte';
	import type { HTMLAnchorAttributes } from 'svelte/elements';

	let {
		href,
		class: klass = '',
		variant = 'plain',
		active = false,
		children,
		...rest
	}: HTMLAnchorAttributes & {
		variant?: 'plain' | 'tab' | 'bottom';
		active?: boolean;
		children?: Snippet;
	} = $props();

	const styled = $derived(variant !== 'plain');
</script>

<a
	{href}
	class="navlink {klass}"
	class:tab={variant === 'tab'}
	class:bottom={variant === 'bottom'}
	class:active={styled && active}
	aria-current={styled && active ? 'page' : undefined}
	{...rest}>{@render children?.()}</a
>

<style>
	.navlink {
		color: inherit;
		text-decoration: none;
		cursor: pointer;
	}
	/* Full-bar-height tab: the active marker is the header's own bottom edge,
	   so there is no radius and no box border. */
	.tab {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		flex: none;
		padding-inline: var(--sp-3);
		color: var(--text-muted);
		box-shadow: inset 0 -2px 0 transparent;
	}
	.tab:hover {
		color: var(--text);
		background: var(--bg-elevated-2);
	}
	.tab.active {
		color: var(--text);
		box-shadow: inset 0 -2px 0 var(--accent);
	}
	/* Bottom-bar item. Fixed chrome, like the header: it deliberately does NOT
	   respond to the font-scale picker, so label and glyph use fixed sizes. */
	.bottom {
		flex: 1;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 2px;
		color: var(--text-faint);
		font-size: 0.6875rem;
		font-weight: var(--fw-medium);
	}
	.bottom.active {
		color: var(--accent);
	}
	.bottom:active {
		background: var(--bg-elevated-2);
	}
</style>
