<script lang="ts">
	// Interface › Theme & look: the per-effect motion picker, behind a labelled
	// "Customize…" button next to the Motion switch.
	import { Button, FX, FX_FAMILIES, fx, Popover, Select, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let { disabled = false }: { disabled?: boolean } = $props();

	const uid = $props.id();
	let reduced = $state(false);
	$effect(() => {
		const mq = globalThis.matchMedia?.('(prefers-reduced-motion: reduce)');
		if (!mq) return;
		reduced = mq.matches;
		const on = (e: MediaQueryListEvent) => (reduced = e.matches);
		mq.addEventListener?.('change', on);
		return () => mq.removeEventListener?.('change', on);
	});
</script>

<Popover label={m.set_motion_tune()} variant="ghost" size="sm" placement="bottom-end" {disabled}>
	{#snippet trigger()}<Text size="sm">{m.set_customize()}</Text>{/snippet}
	<div class="panel" data-testid="motion-customize">
		<div class="grid">
			{#each FX_FAMILIES as family (family)}
				<label class="name" for="{uid}-{family}">{FX[family].label}</label>
				<Select
					id="{uid}-{family}"
					size="sm"
					value={fx.settings[family]}
					options={FX[family].variants.map((v) => ({ value: v.id, label: v.label }))}
					onchange={(e) => fx.set(family, (e.currentTarget as HTMLSelectElement).value)}
				/>
			{/each}
		</div>
		{#if reduced}
			<Text size="xs" tone="faint" as="p">{m.settings_motion_reduced_hint()}</Text>
		{/if}
		<div class="foot">
			<Button size="sm" variant="ghost" disabled={fx.isDefault} onclick={() => fx.reset()}>
				{m.settings_motion_reset()}
			</Button>
		</div>
	</div>
</Popover>

<style>
	.panel {
		display: grid;
		gap: var(--sp-2);
		width: min(19rem, calc(100vw - var(--sp-6)));
		padding: var(--sp-3);
	}
	.grid {
		display: grid;
		grid-template-columns: auto minmax(0, 1fr);
		align-items: center;
		gap: var(--sp-2) var(--sp-3);
	}
	.name {
		font-size: var(--fs-sm);
	}
	.foot {
		display: flex;
		justify-content: flex-end;
	}
</style>
