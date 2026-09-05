<script lang="ts">
	import type { Snippet } from 'svelte';
	import { Button, Input, SegmentedControl, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import type { Preset } from '@bindings/Preset';
	import {
		applyPreset,
		getKnob,
		overriddenCount,
		setKnob,
		type Knob,
		type KnobGroup
	} from './knobs.logic';

	let {
		groups,
		settings = $bindable({}),
		preset,
		loading = false,
		failed = false,
		children
	}: {
		groups: KnobGroup[];
		settings?: Record<string, unknown>;
		preset?: Preset;
		loading?: boolean;
		failed?: boolean;
		/** Page-specific controls rendered above the catalog rows. */
		children?: Snippet;
	} = $props();

	const TRI = [
		{ value: '', label: m.providers_opt_default() },
		{ value: 'true', label: m.providers_opt_on() },
		{ value: 'false', label: m.providers_opt_off() }
	];
	const FLAG = [
		{ value: '', label: m.providers_opt_default() },
		{ value: '1', label: m.providers_opt_on() }
	];

	const knobs = $derived(groups.flatMap((g) => g.knobs));
	const overridden = $derived(overriddenCount(settings, knobs));
	const enumOptions = (k: Knob) => [
		{ value: '', label: m.providers_opt_default() },
		...(k.values ?? []).map((v) => ({ value: v, label: v }))
	];
	const set = (k: Knob, v: string) => {
		settings = setKnob(settings, k, v);
	};
</script>

<div class="page">
	<div class="head">
		<Text as="span" size="xs" tone="faint">
			{m.provider_drawer_overridden({ n: overridden })}
		</Text>
		<span class="spacer"></span>
		{#if knobs.length}
			<Button size="sm" disabled={!preset} onclick={() => (settings = applyPreset(settings, preset, knobs))}>
				{m.providers_quiet_defaults()}
			</Button>
		{/if}
	</div>

	{#if children}{@render children()}{/if}

	{#if loading}
		<Text as="div" tone="faint" size="xs">{m.providers_catalog_loading()}</Text>
	{:else if failed}
		<Text as="div" tone="faint" size="xs">{m.providers_catalog_load_failed()}</Text>
	{/if}

	{#each groups as group (group.title)}
		<div class="group">
			<div class="group-title"><Text as="span" tone="faint" size="xs">{group.title}</Text></div>
			{#each group.knobs as k (k.id)}
				{@const value = getKnob(settings, k)}
				<div class="row" class:overridden={value !== ''}>
					<div class="meta">
						<Text as="div" size="sm">
							{k.label}
							{#if k.care}<span class="care" title={m.providers_care_title()}>{m.providers_care()}</span>{/if}
						</Text>
						<div class="key"><Text as="span" tone="faint" size="xs">{k.sub}</Text></div>
					</div>
					<div class="control">
						{#if k.control === 'tristate' || k.control === 'toggle' || k.control === 'enum'}
							<SegmentedControl
								size="sm"
								label={k.label}
								options={k.control === 'tristate'
									? TRI
									: k.control === 'toggle'
										? FLAG
										: enumOptions(k)}
								bind:value={() => value, (v) => set(k, v)}
							/>
						{:else}
							<Input
								{value}
								oninput={(e: Event) => set(k, (e.currentTarget as HTMLInputElement).value)}
								type={k.control === 'number' ? 'number' : 'text'}
								size="sm"
								mono
								placeholder={m.providers_opt_default()}
								aria-label={k.label}
							/>
						{/if}
					</div>
				</div>
			{/each}
		</div>
	{/each}
</div>

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.head {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.spacer {
		flex: 1;
	}
	.group {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	.group-title {
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}
	.row {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(0, auto);
		gap: var(--sp-2) var(--sp-3);
		align-items: center;
		padding: var(--sp-1) var(--sp-2);
		border-radius: var(--r-sm);
	}
	.row.overridden {
		background: color-mix(in srgb, var(--accent) 7%, transparent);
	}
	.meta {
		min-width: 0;
	}
	.key {
		font-family: var(--font-mono, monospace);
		overflow-wrap: anywhere;
	}
	.control {
		justify-self: end;
		min-width: 0;
	}
	.care {
		display: inline-block;
		margin-left: var(--sp-1);
		padding: 0 0.35em;
		border-radius: var(--r-sm);
		font-size: var(--fs-xs);
		background: color-mix(in srgb, var(--warn) 18%, transparent);
		color: var(--warn);
	}
	@container (max-width: 26rem) {
		.row {
			grid-template-columns: minmax(0, 1fr);
		}
		.control {
			justify-self: start;
		}
	}
</style>
