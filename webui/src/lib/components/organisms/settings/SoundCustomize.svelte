<script lang="ts">
	// Interface › Theme & look: volume and a preview for the UI sound effects,
	// behind a labelled "Customize…" button next to the Sound effects switch.
	import { Button, Popover, Slider, sound, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let { disabled = false }: { disabled?: boolean } = $props();

	const percent = $derived(Math.round(sound.volume * 100));
</script>

<Popover label={m.set_sound_tune()} variant="ghost" size="sm" placement="bottom-end" {disabled}>
	{#snippet trigger()}<Text size="sm">{m.set_customize()}</Text>{/snippet}
	<div class="panel" data-testid="sound-customize">
		<div class="row">
			<Slider
				value={sound.volume}
				min={0}
				max={1}
				step={0.05}
				label={m.settings_sound_volume()}
				aria-valuetext="{percent}%"
				oninput={(e) => sound.setVolume(Number((e.currentTarget as HTMLInputElement).value))}
			/>
			<Text size="sm" class="num">{percent}%</Text>
		</div>
		{#if sound.muted === 'reduced-motion'}
			<Text size="xs" tone="faint" as="p">{m.settings_sound_reduced_hint()}</Text>
		{/if}
		<div class="foot">
			<Button size="sm" variant="ghost" sound={false} onclick={() => sound.play('success')}>
				{m.settings_sound_preview()}
			</Button>
		</div>
	</div>
</Popover>

<style>
	.panel {
		display: grid;
		gap: var(--sp-2);
		width: min(17rem, calc(100vw - var(--sp-6)));
		padding: var(--sp-3);
	}
	.row {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.foot {
		display: flex;
		justify-content: flex-end;
	}
</style>
