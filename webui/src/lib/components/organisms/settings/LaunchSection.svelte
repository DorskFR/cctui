<script lang="ts">
	// Sessions › Launch & profiles: the saved launch profiles, the Claude
	// harness mode new sessions start in, and session-name emoji. The mode
	// reaches a connected daemon within ~1 s.
	import { OptionButton, Switch, Text } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import ProfilesEditor from './ProfilesEditor.svelte';
	import { settings, type HarnessMode } from '$lib/settings.svelte';
	import { m } from '$lib/paraglide/messages';

	let { active = false }: { active?: boolean } = $props();

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
</script>

<SettingSection id="profiles" icon="▶" title={m.set_launch_title()} description={m.set_launch_desc()}>
	<SettingGroup title={m.set_profiles_group()}>
		{#if active}
			<ProfilesEditor />
		{/if}
	</SettingGroup>
	<SettingGroup>
		<SettingRow label={m.settings_harness_execution_label()} wide selfLabelled>
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
		<SettingRow label={m.settings_session_emoji_label()} help={m.settings_session_emoji_help()}>
			<Switch
				bind:checked={() => settings.sessionEmojiPrefix, (v) => settings.setSessionEmojiPrefix(v)}
				label={m.settings_session_emoji_label()}
			/>
		</SettingRow>
	</SettingGroup>
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
