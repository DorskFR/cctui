<script lang="ts">
	import type { GaugeTone } from './usage-gauge.logic.ts';

	let { pct, tone, width = 32 }: { pct: number; tone: GaugeTone; width?: number } = $props();
</script>

<span class="meter" data-tone={tone} style:width="{width}px" aria-hidden="true">
	<span class="fill" style:width="{Math.min(100, Math.max(0, pct))}%"></span>
</span>

<style>
	/* px, never rem: the meter also lives in the px-pinned header. */
	.meter {
		display: block;
		flex: none;
		height: 6px;
		border-radius: 3px;
		background: color-mix(in srgb, var(--text-faint) 30%, transparent);
		overflow: hidden;
	}
	.fill {
		display: block;
		height: 100%;
		border-radius: 3px;
		background: var(--ok);
	}
	.meter[data-tone='warn'] .fill {
		background: var(--warn);
	}
	.meter[data-tone='danger'] .fill {
		background: var(--danger);
	}
</style>
