<script lang="ts">
	// Settings › Privacy: daemon-side secret redaction. The switch toggles live
	// scrubbing; the textarea holds one extra regex per line, layered on the
	// daemon's compiled defaults. The server validates each regex on save.
	import { Badge, Switch, Text, Textarea } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import { settings } from '$lib/settings.svelte';
	import { BUILTIN_SCRUB_CATEGORIES } from './settings.logic';
	import { m } from '$lib/paraglide/messages';

	const scrubEnabled = $derived(settings.secretScrubEnabled);
	const scrubPatternsText = $derived(settings.secretScrubPatterns.map((p) => p.regex).join('\n'));
	function setScrubPatternsText(text: string) {
		const patterns = text
			.split('\n')
			.map((r) => r.trim())
			.filter((r) => r.length > 0)
			.map((regex) => ({ name: 'custom', regex, enabled: true }));
		settings.setSecretScrubPatterns(patterns);
	}
</script>

<SettingSection
	id="privacy"
	icon="◈"
	title={m.settings_nav_privacy()}
	description={m.settings_redaction_help()}
>
	<SettingGroup>
		<SettingRow label={m.settings_redaction_enable_label()} help={m.settings_redaction_enable_help()} server>
			<Switch
				bind:checked={() => scrubEnabled, (v) => settings.setSecretScrubEnabled(v)}
				label={m.settings_redaction_enable_label()}
			/>
		</SettingRow>
		<SettingRow
			label={m.settings_redaction_patterns_label()}
			help={m.settings_redaction_patterns_hint()}
			wide
		>
			<Textarea
				mono
				autoresize
				rows={6}
				style="width:100%;min-height:9rem"
				value={scrubPatternsText}
				placeholder={'ACME-[0-9]{6}\nMYCORP_[A-Za-z0-9]{20,}'}
				onchange={(e) => setScrubPatternsText((e.currentTarget as HTMLTextAreaElement).value)}
			/>
		</SettingRow>
		<details class="defaults" data-setting-row>
			<summary>
				<Text size="sm" tone="muted">
					{m.settings_redaction_builtins_count({ count: BUILTIN_SCRUB_CATEGORIES.length })}
				</Text>
			</summary>
			<div class="chips">
				{#each BUILTIN_SCRUB_CATEGORIES as c (c)}
					<Badge mono size="sm" border>{c}</Badge>
				{/each}
			</div>
		</details>
	</SettingGroup>
</SettingSection>

<style>
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
</style>
