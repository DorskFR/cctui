<script lang="ts">
	import { Input, Switch, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import type { AutoLimitReset } from '$lib/components/organisms/accounts/provider-drawer/auto-limit-reset.logic';

	let {
		value = $bindable(),
		family
	}: {
		value: AutoLimitReset;
		/** `openai` shows the Codex knobs, `anthropic` the Claude one. */
		family: 'openai' | 'anthropic';
	} = $props();
	const idBase = $derived(`auto-limit-reset-${family}`);
</script>

<div class="auto-reset">
	<div class="line">
		<Switch
			bind:checked={() => value.enabled, (v) => (value = { ...value, enabled: v })}
			label={m.auto_limit_reset_enabled()}
			labelVisible
			size="sm"
		/>
	</div>
	<div class="knobs">
		{#if family === 'openai'}
			<label class="knob" for="{idBase}-used">
				<Text as="span" size="xs" tone="muted">{m.auto_limit_reset_used_label()}</Text>
				<Input
					id="{idBase}-used"
					type="number"
					min="0"
					max="100"
					step="1"
					size="sm"
					mono
					width="64px"
					disabled={!value.enabled}
					bind:value={value.used_pct}
				/>
			</label>
			<label class="knob" for="{idBase}-expires">
				<Text as="span" size="xs" tone="muted">{m.auto_limit_reset_expires_label()}</Text>
				<Input
					id="{idBase}-expires"
					type="number"
					min="0"
					step="1"
					size="sm"
					mono
					width="64px"
					disabled={!value.enabled}
					bind:value={value.expires_within_hours}
				/>
			</label>
		{:else}
			<label class="knob" for="{idBase}-weekly">
				<Text as="span" size="xs" tone="muted">{m.auto_limit_reset_weekly_label()}</Text>
				<Input
					id="{idBase}-weekly"
					type="number"
					min="0"
					max="100"
					step="1"
					size="sm"
					mono
					width="64px"
					disabled={!value.enabled}
					bind:value={value.weekly_max_pct}
				/>
			</label>
		{/if}
	</div>
	<Text as="p" tone="faint" size="xs">
		{family === 'openai' ? m.auto_limit_reset_help_codex() : m.auto_limit_reset_help_claude()}
	</Text>
</div>

<style>
	.auto-reset {
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
	.knobs {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-3);
	}
	.knob {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
	}
</style>
