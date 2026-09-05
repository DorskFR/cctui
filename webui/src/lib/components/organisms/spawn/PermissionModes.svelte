<script lang="ts">
	import type { PermissionMode } from '@bindings/PermissionMode';
	import { Text } from '@dorsk/tsumikit';
	import { modes } from '$lib/components/organisms/spawn/options';
	import { m } from '$lib/paraglide/messages';

	let {
		value,
		onpick
	}: { value: string | null; onpick: (v: PermissionMode) => void } = $props();
</script>

<div class="modes">
	<Text size="xs" tone="muted">{m.spawn_permission_mode_label()}</Text>
	<div class="grid" role="radiogroup" aria-label={m.spawn_permission_mode_label()}>
		{#each modes as md (md.v)}
			<button
				type="button"
				class="mode"
				class:on={value === md.v}
				data-mode={md.v}
				role="radio"
				aria-checked={value === md.v}
				title={md.hint}
				onclick={() => onpick(md.v)}
			>
				<span class="mode-label">{md.label}</span>
				<span class="mode-hint">{md.hint}</span>
			</button>
		{/each}
	</div>
</div>

<style>
	.modes {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		container-type: inline-size;
	}
	.grid {
		display: grid;
		grid-template-columns: repeat(2, 1fr);
		gap: var(--sp-2);
	}
	.mode {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
		padding: var(--sp-2) var(--sp-3);
		text-align: left;
		border: 1px solid var(--border);
		border-radius: var(--r-md);
		background: var(--bg-elevated);
		color: var(--text-muted);
		cursor: pointer;
	}
	.mode:hover {
		background: var(--bg-elevated-2);
	}
	.mode.on {
		border-color: var(--accent);
		color: var(--text);
	}
	.mode[data-mode='yolo'].on,
	.mode[data-mode='whip'].on {
		border-color: var(--danger);
	}
	.mode-label {
		font-size: var(--fs-sm);
		font-weight: var(--fw-semibold);
		white-space: nowrap;
	}
	.mode[data-mode='yolo'].on .mode-label,
	.mode[data-mode='whip'].on .mode-label {
		color: var(--danger);
	}
	.mode-hint {
		font-size: var(--fs-xs);
		line-height: 1.25;
		color: var(--text-faint);
	}
	/* Too narrow for a two-up card: drop to one compact row of four, which is
	   why every label must stay on a single line. */
	@container (max-width: 22rem) {
		.grid {
			grid-template-columns: repeat(4, 1fr);
			gap: var(--sp-1);
		}
		.mode {
			align-items: center;
			padding: var(--sp-1) 2px;
			text-align: center;
		}
		.mode-label {
			font-size: var(--fs-xs);
		}
		.mode-hint {
			display: none;
		}
	}
</style>
