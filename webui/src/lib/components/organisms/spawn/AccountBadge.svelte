<script lang="ts">
	import type { BadgeLook } from './accountBadge';

	let { look, size = 'card' }: { look: BadgeLook; size?: 'card' | 'row' } = $props();
</script>

<span
	class="badge {size}"
	class:tinted={look.hue !== null}
	data-kind={look.kind}
	data-journey="account-badge"
	aria-hidden="true"
	style:--mh={look.hue ?? undefined}>{look.glyph}</span
>

<style>
	.badge {
		flex: none;
		display: inline-grid;
		place-items: center;
		box-sizing: border-box;
		border-radius: var(--r-md);
		background: var(--bg-elevated-2);
		color: var(--text-muted);
		box-shadow: inset 0 0 0 1px var(--border);
		font-weight: var(--fw-semibold);
		line-height: 1;
		user-select: none;
	}
	.badge.tinted {
		background: hsl(var(--mh) var(--mach-bg-sl));
		color: hsl(var(--mh) var(--mach-fg-sl));
		box-shadow: inset 0 0 0 1px hsl(var(--mh) var(--mach-border-sl));
	}
	.card {
		width: 36px;
		height: 36px;
		font-size: var(--fs-md);
	}
	.row {
		width: 30px;
		height: 30px;
		border-radius: var(--r-sm);
		font-size: var(--fs-sm);
	}
</style>
