<script lang="ts">
	import { Field, Input, Switch, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import type { UsageNotices } from '$lib/queries';

	let { value = $bindable({ enabled: false, step_pct: 10 }) }: { value: UsageNotices } = $props();

	const stepId = 'usage-notices-step';
</script>

<div class="usage-notices">
	<Text as="div" tone="muted" size="sm">{m.usage_notices_label()}</Text>
	<Text as="div" tone="faint" size="xs">{m.usage_notices_help()}</Text>
	<div class="row">
		<Switch
			checked={value.enabled}
			label={m.usage_notices_enabled()}
			onclick={() => (value = { ...value, enabled: !value.enabled })}
		/>
		<Field label={m.usage_notices_step_label()} for={stepId}>
			<Input
				id={stepId}
				type="number"
				min="1"
				max="100"
				step="1"
				disabled={!value.enabled}
				bind:value={value.step_pct}
			/>
		</Field>
	</div>
</div>

<style>
	.usage-notices {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.row {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: var(--sp-2);
		align-items: end;
	}
</style>
