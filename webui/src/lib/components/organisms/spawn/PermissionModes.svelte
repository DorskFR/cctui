<script lang="ts">
	import type { PermissionMode } from '@bindings/PermissionMode';
	import { AutoGrid, OptionButton, Text, type Tone } from '@dorsk/tsumikit';
	import { modesFor } from '#lib/components/organisms/spawn/options.ts';
	import { m } from '#lib/paraglide/messages.js';

	let {
		value,
		onpick,
		harness = null
	}: {
		value: string | null;
		onpick: (v: PermissionMode) => void;
		/** Narrows the cards to the postures this harness can express. */
		harness?: string | null;
	} = $props();

	const modes = $derived(modesFor(harness));

	// Per-mode accent: ask = green (safe), auto = blue (sandboxed),
	// yolo = red (no prompts, full access), whip = violet (yolo and never stalls).
	const modeTone: Record<string, Tone> = { ask: 'ok', auto: 'info', yolo: 'danger', whip: 'accent' };
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
				tone={modeTone[md.v]}
				style={md.v === 'whip' ? '--accent: var(--c-violet)' : undefined}
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
