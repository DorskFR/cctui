<script lang="ts">
	// Settings › Execution: the Claude harness mode (three radio cards) and the
	// whip-mode stall phrases. Both live on the server: the mode reaches a
	// connected daemon within ~1 s, the phrases apply at the next spawn.
	import { Badge, OptionButton, SegmentedControl, Text, Textarea } from '@dorsk/tsumikit';
	import type { SegmentOption } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import { settings, type HarnessMode, type WhipMode } from '$lib/settings.svelte';
	import { BUILTIN_STALL_PHRASES } from './settings.logic';
	import { m } from '$lib/paraglide/messages';

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

	// `extend` appends to the daemon's compiled defaults; `replace` swaps them.
	// One phrase per line; the server trims/lowercases/dedupes/caps on save.
	const whip = $derived(settings.whipStopPhrases);
	const whipPhrasesText = $derived(whip.phrases.join('\n'));
	function setWhipPhrasesText(text: string) {
		const phrases = text
			.split('\n')
			.map((p) => p.trim())
			.filter((p) => p.length > 0);
		settings.setWhipStopPhrases({ phrases });
	}
	const whipModeOptions: SegmentOption[] = [
		{ value: 'extend', label: m.settings_whip_mode_extend_short() },
		{ value: 'replace', label: m.settings_whip_mode_replace_short() }
	];
</script>

<SettingSection
	id="execution"
	icon="▶"
	title={m.settings_nav_execution()}
	description={m.settings_execution_desc()}
>
	<SettingGroup>
		<SettingRow label={m.settings_harness_execution_label()} server wide selfLabelled>
			<div class="radios" role="radiogroup" aria-label={m.settings_harness_execution_label()}>
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
	</SettingGroup>

	<SettingGroup title={m.settings_whip_title()}>
		<SettingRow
			label={m.settings_whip_phrase_list_label()}
			help={whip.mode === 'replace'
				? m.settings_whip_mode_replace_help()
				: m.settings_whip_mode_extend_help()}
			server
			selfLabelled
		>
			<SegmentedControl
				options={whipModeOptions}
				label={m.settings_whip_phrase_list_label()}
				control
				bind:value={
					() => whip.mode, (v) => settings.setWhipStopPhrases({ mode: v as WhipMode })
				}
			/>
		</SettingRow>
		<SettingRow label={m.settings_whip_phrases_field_label()} help={m.settings_whip_phrases_help()} wide>
			<Textarea
				mono
				autoresize
				rows={3}
				style="width:100%"
				value={whipPhrasesText}
				placeholder={'pour une autre session\nprêt pour ta relecture'}
				onchange={(e) => setWhipPhrasesText((e.currentTarget as HTMLTextAreaElement).value)}
			/>
		</SettingRow>
		<SettingRow label={m.settings_whip_guidance_label()} help={m.settings_whip_guidance_hint()} wide>
			<Textarea
				autoresize
				rows={2}
				style="width:100%"
				value={whip.guidance}
				onchange={(e) =>
					settings.setWhipStopPhrases({
						guidance: (e.currentTarget as HTMLTextAreaElement).value.trim()
					})}
			/>
		</SettingRow>
		<details class="defaults" data-setting-row>
			<summary>
				<Text size="sm" tone="muted">
					{m.settings_whip_defaults_count({ count: BUILTIN_STALL_PHRASES.length })}
				</Text>
			</summary>
			<div class="chips">
				{#each BUILTIN_STALL_PHRASES as p (p)}
					<Badge mono size="sm" border>{p}</Badge>
				{/each}
			</div>
		</details>
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
	.defaults {
		border-top: 1px solid var(--border);
	}
	.defaults summary {
		padding: var(--sp-2) var(--sp-4);
		cursor: pointer;
	}
	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-1);
		padding: 0 var(--sp-4) var(--sp-3);
	}
	@media (max-width: 47.999rem) {
		.radios {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
