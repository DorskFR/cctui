<script lang="ts">
	import { Input, Switch, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import type { UsageNotices } from '$lib/queries';

	let { value = $bindable({ enabled: false, step_pct: 10 }) }: { value: UsageNotices } = $props();
	const stepId = 'usage-notices-step';
</script>

<div class="usage-notices">
	<div class="line">
		<Switch
			checked={value.enabled}
			label={m.usage_notices_enabled()}
			labelVisible
			size="sm"
			onclick={() => (value = { ...value, enabled: !value.enabled })}
		/>
		<span class="spacer"></span>
		<label class="step" for={stepId}>
			<Text as="span" size="xs" tone="muted">{m.usage_notices_step_label()}</Text>
			<Input
				id={stepId}
				type="number"
				min="1"
				max="100"
				step="1"
				size="sm"
				mono
				width="56px"
				disabled={!value.enabled}
				bind:value={value.step_pct}
			/>
		</label>
	</div>
	<Text as="p" tone="faint" size="xs">{m.usage_notices_help()}</Text>
</div>

<style>
	.usage-notices {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		padding-top: var(--sp-2);
		border-top: 1px solid var(--border);
	}
	.line {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.spacer {
		flex: 1;
	}
	.step {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
	}
</style>
