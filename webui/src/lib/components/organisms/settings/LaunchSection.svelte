<script lang="ts">
	import { OptionButton, Switch, Text } from '@dorsk/tsumikit';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import SettingSection from '#lib/components/molecules/SettingSection.svelte';
	import MacrosGroup from './MacrosGroup.svelte';
	import ProfilesGroup from './ProfilesGroup.svelte';
	import { settings, type HarnessMode } from '#lib/settings.svelte.ts';
	import { hasFeature } from '#lib/features.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';

	// The harness mode reaches a connected daemon within ~1 s.
	const harnessMode = $derived(settings.harnessMode);
	const harnessOpts: { v: HarnessMode; label: string; help: string }[] = [
		{ v: 'bg', label: m.settings_harness_bg_label(), help: m.settings_harness_bg_help() },
		{ v: 'sdk', label: m.settings_harness_sdk_label(), help: m.settings_harness_sdk_help() },
		{
			v: 'oneshot',
			label: m.settings_harness_oneshot_label(),
			help: m.settings_harness_oneshot_help()
		}
	];
	const sessionEmojiPrefix = $derived(settings.sessionEmojiPrefix);
</script>

<SettingSection
	id="launch"
	icon="▶"
	title={m.settings_sec_launch()}
	description={hasFeature('machine_spawn') ? m.settings_execution_desc() : undefined}
>
	{#if hasFeature('machine_spawn') && hasFeature('profiles')}
		<ProfilesGroup />
	{/if}
	<SettingGroup>
		<SettingRow label={m.settings_harness_execution_label()} server wide selfLabelled>
			<div class="radios" role="radiogroup" aria-label={m.settings_harness_execution_label()} data-journey="harness-mode">
				{#each harnessOpts as o (o.v)}
					<OptionButton
						block
						align="start"
						selected={harnessMode === o.v}
						role="radio"
						aria-checked={harnessMode === o.v}
						onclick={() => settings.setHarnessMode(o.v)}
					>
						<span class="radio-body">
							<Text weight="semibold" size="sm" as="span">{o.label}</Text>
							<Text size="xs" tone="faint" as="span">{o.help}</Text>
						</span>
					</OptionButton>
				{/each}
			</div>
		</SettingRow>
		<SettingRow label={m.settings_session_emoji_label()} help={m.settings_session_emoji_help()} server>
			<Switch
				bind:checked={() => sessionEmojiPrefix, (v) => settings.setSessionEmojiPrefix(v)}
				label={m.settings_session_emoji_label()}
			/>
		</SettingRow>
	</SettingGroup>
	{#if hasFeature('machine_spawn')}
		<MacrosGroup />
	{/if}
</SettingSection>

<style>
	.radios {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: var(--sp-2);
	}
	.radio-body {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		text-align: left;
	}
	@media (max-width: 47.999rem) {
		.radios {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
