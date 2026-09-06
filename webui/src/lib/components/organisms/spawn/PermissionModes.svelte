<script lang="ts">
	import type { PermissionMode } from '@bindings/PermissionMode';
	import { AutoGrid, OptionButton, Text } from '@dorsk/tsumikit';
	import { modes } from '$lib/components/organisms/spawn/options';
	import { m } from '$lib/paraglide/messages';

	let {
		value,
		onpick
	}: { value: string | null; onpick: (v: PermissionMode) => void } = $props();

	// Per-mode accent: ask = green (safe), auto = blue (sandboxed),
	// yolo = red (no prompts, full access), whip = violet (yolo and never stalls).
	const modeAccent: Record<string, string> = {
		ask: 'var(--c-green)',
		auto: 'var(--c-blue)',
		yolo: 'var(--c-red)',
		whip: 'var(--c-violet)'
	};
</script>

<div class="modes">
	<Text size="xs" tone="muted">{m.spawn_permission_mode_label()}</Text>
	<AutoGrid min="8rem" maxCols={2} gap="var(--sp-2)" align="stretch" role="radiogroup" aria-label={m.spawn_permission_mode_label()}>
		{#each modes as md (md.v)}
			<OptionButton
				block
				align="start"
				selected={value === md.v}
				role="radio"
				aria-checked={value === md.v}
				data-mode={md.v}
				style={`--opt-accent: ${modeAccent[md.v]}`}
				title={md.hint}
				onclick={() => onpick(md.v)}
			>
				<span class="mode-body">
					<Text weight="semibold" size="sm" as="span">{md.label}</Text>
					<span class="mode-hint"><Text size="xs" tone="faint" as="span">{md.hint}</Text></span>
				</span>
			</OptionButton>
		{/each}
	</AutoGrid>
</div>

<style>
	.modes {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		container-type: inline-size;
	}
	/* Cards stretch to the tallest sibling of their row (align="stretch"), so a
	   one-line hint next to a two-line one no longer leaves a shorter card. */
	.mode-body {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}
</style>
