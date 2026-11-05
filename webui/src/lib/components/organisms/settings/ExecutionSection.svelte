<script lang="ts">
	// Sessions › Automation: whip mode's stall phrases (applied at the next
	// spawn) and auto-archiving of dispatched sessions.
	import { Badge, Disclosure, Input, SegmentedControl, Switch, Text, Textarea } from '@dorsk/tsumikit';
	import type { SegmentOption } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import { settings, type WhipMode } from '$lib/settings.svelte';
	import { BUILTIN_STALL_PHRASES } from './settings.logic';
	import { m } from '$lib/paraglide/messages';

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

	const dispatchArchive = $derived(settings.dispatchAutoArchive);
	function onGrace(e: Event) {
		settings.setDispatchAutoArchiveGraceSecs((e.currentTarget as HTMLInputElement).valueAsNumber);
	}
	function onKeepProfiles(e: Event) {
		settings.setDispatchAutoArchiveKeepProfiles((e.currentTarget as HTMLInputElement).value.split(','));
	}
</script>

<SettingSection id="automation" icon="recycle" title={m.set_automation_title()}>
	<SettingGroup title={m.settings_whip_title()}>
		<SettingRow
			label={m.settings_whip_phrase_list_label()}
			help={whip.mode === 'replace'
				? m.settings_whip_mode_replace_help()
				: m.settings_whip_mode_extend_help()}
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
				rows={6}
				style="width:100%;min-height:9rem;font-family:var(--font-mono)"
				value={whipPhrasesText}
				placeholder={'pour une autre session\nprêt pour ta relecture'}
				onchange={(e) => setWhipPhrasesText((e.currentTarget as HTMLTextAreaElement).value)}
			/>
		</SettingRow>
		<SettingRow label={m.settings_whip_guidance_label()} help={m.settings_whip_guidance_hint()} wide>
			<Textarea
				autoresize
				rows={4}
				style="width:100%;min-height:6rem"
				value={whip.guidance}
				onchange={(e) =>
					settings.setWhipStopPhrases({
						guidance: (e.currentTarget as HTMLTextAreaElement).value.trim()
					})}
			/>
		</SettingRow>
		<div class="defaults" data-setting-row>
			<Disclosure>
				{#snippet header()}
					<Text size="sm" tone="muted">
						{m.settings_whip_defaults_count({ count: BUILTIN_STALL_PHRASES.length })}
					</Text>
				{/snippet}
				<div class="chips">
					{#each BUILTIN_STALL_PHRASES as p (p)}
						<Badge size="sm" border><span class="code">{p}</span></Badge>
					{/each}
				</div>
			</Disclosure>
		</div>
	</SettingGroup>

	<SettingGroup title={m.settings_group_dispatch()}>
		<SettingRow label={m.settings_dispatch_archive_label()} help={m.settings_dispatch_archive_help()}>
			<Switch
				bind:checked={() => dispatchArchive, (v) => settings.setDispatchAutoArchive(v)}
				label={m.settings_dispatch_archive_label()}
			/>
		</SettingRow>
		<SettingRow
			label={m.settings_dispatch_archive_grace_label()}
			help={m.settings_dispatch_archive_grace_help()}
			disabled={!dispatchArchive}
		>
			<Input
				type="number"
				min="0"
				max="3600"
				step="1"
				value={settings.dispatchAutoArchiveGraceSecs}
				disabled={!dispatchArchive}
				onchange={onGrace}
				aria-label={m.settings_dispatch_archive_grace_label()}
			/>
		</SettingRow>
		<SettingRow
			label={m.settings_dispatch_archive_keep_label()}
			help={m.settings_dispatch_archive_keep_help()}
			disabled={!dispatchArchive}
		>
			<Input
				style="width:100%"
				value={settings.dispatchAutoArchiveKeepProfiles.join(', ')}
				disabled={!dispatchArchive}
				onchange={onKeepProfiles}
				aria-label={m.settings_dispatch_archive_keep_label()}
			/>
		</SettingRow>
	</SettingGroup>
</SettingSection>

<style>
	.defaults {
		border-top: 1px solid var(--border);
	}
	.code {
		font-family: var(--font-mono);
	}
	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-1);
		padding: 0 var(--sp-4) var(--sp-3);
	}
</style>
