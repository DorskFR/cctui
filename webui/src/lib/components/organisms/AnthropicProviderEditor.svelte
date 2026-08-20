<script lang="ts">
	import { Field, Select, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		settings = $bindable({})
	}: {
		/** Mirrors the provider's `provider_settings` blob. */
		settings?: Record<string, unknown>;
	} = $props();

	const DISPLAYS = ['summarized', 'omitted'];

	const display = $derived(
		typeof settings.thinking_display === 'string' ? settings.thinking_display : ''
	);

	function setDisplay(v: string) {
		settings = { ...settings, thinking_display: v === '' ? null : v };
	}
</script>

<div class="block">
	<Text as="div" tone="muted" size="sm">{m.anthropic_settings_label()}</Text>
	<Text as="div" tone="faint" size="xs">{m.anthropic_settings_help()}</Text>
	<Field label={m.anthropic_thinking_display_label()}>
		<Select
			value={display}
			onchange={(e) => setDisplay((e.currentTarget as HTMLSelectElement).value)}
		>
			<option value="">{m.anthropic_thinking_display_unset()}</option>
			{#each DISPLAYS as d (d)}
				<option value={d}>{d}</option>
			{/each}
		</Select>
	</Field>
	<Text as="div" tone="faint" size="xs">{m.anthropic_thinking_display_help()}</Text>
</div>

<style>
	.block {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
</style>
