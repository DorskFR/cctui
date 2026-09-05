<script lang="ts">
	import type { PermissionMode } from '@bindings/PermissionMode';
	import { OptionButton, Text } from '@dorsk/tsumikit';
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
			<OptionButton
				block
				align="start"
				selected={value === md.v}
				role="radio"
				aria-checked={value === md.v}
				data-mode={md.v}
				title={md.hint}
				onclick={() => onpick(md.v)}
			>
				<span class="mode-body">
					<Text weight="semibold" size="sm" as="span">{md.label}</Text>
					<span class="mode-hint"><Text size="xs" tone="faint" as="span">{md.hint}</Text></span>
				</span>
			</OptionButton>
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
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--sp-2);
	}
	.mode-body {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}
	/* Too narrow for a two-up card: drop to one compact row of four, so every
	   label has to survive on a single line. */
	@container (max-width: 22rem) {
		.grid {
			grid-template-columns: repeat(4, minmax(0, 1fr));
			gap: var(--sp-1);
		}
		.mode-hint {
			display: none;
		}
	}
</style>
