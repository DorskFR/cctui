<script lang="ts">
	// Settings › Privacy: daemon-side secret redaction. The switch toggles live
	// scrubbing; the textarea holds one extra regex per line, layered on the
	// daemon's compiled defaults. A line that does not compile is flagged here and
	// withheld from the save; the server stays authoritative (Rust regex syntax).
	import { Button, Callout, Switch, Text, Textarea } from '@dorsk/tsumikit';
	import { parseScrubPatterns } from './scrub-patterns';
	import DetectorList from '$lib/components/molecules/DetectorList.svelte';
	import RescrubPanel from '$lib/components/molecules/RescrubPanel.svelte';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import { settings } from '$lib/settings.svelte';
	import { m } from '$lib/paraglide/messages';

	const scrubEnabled = $derived(settings.secretScrubEnabled);
	// The draft holds what was typed, including a line that doesn't compile: only
	// the valid lines are handed to the store, so one bad pattern can't make every
	// later settings save fail.
	let draft = $state<string | null>(null);
	const scrubPatternsText = $derived(
		draft ?? settings.secretScrubPatterns.map((p) => p.regex).join('\n')
	);
	const issues = $derived(parseScrubPatterns(scrubPatternsText).issues);
	function setScrubPatternsText(text: string) {
		draft = text;
		settings.setSecretScrubPatterns(parseScrubPatterns(text).patterns);
	}
</script>

<SettingSection
	id="privacy"
	icon="◈"
	title={m.settings_nav_privacy()}
	description={m.settings_redaction_help()}
>
	{#if !scrubEnabled}
		<Callout tone="warn" title={m.settings_redaction_off_title()}>
			{m.settings_redaction_off_body()}
			{#snippet actions()}
				<Button size="sm" tone="warn" onclick={() => settings.setSecretScrubEnabled(true)}>
					{m.settings_redaction_off_action()}
				</Button>
			{/snippet}
		</Callout>
	{/if}
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
				data-journey="redact-patterns"
				mono
				autoresize
				rows={6}
				style="width:100%;min-height:9rem"
				value={scrubPatternsText}
				placeholder={'ACME-[0-9]{6}\nMYCORP_[A-Za-z0-9]{20,}'}
				oninput={(e) => (draft = (e.currentTarget as HTMLTextAreaElement).value)}
				onchange={(e) => setScrubPatternsText((e.currentTarget as HTMLTextAreaElement).value)}
			/>
			{#each issues as issue (issue.line)}
				<Text size="xs" tone="danger" as="div">
					{m.settings_redaction_patterns_invalid({ line: issue.line, message: issue.message })}
				</Text>
			{/each}
			{#if settings.saveError}
				<Text size="xs" tone="danger" as="div">{settings.saveError}</Text>
			{/if}
		</SettingRow>
		<RescrubPanel />
		<DetectorList />
	</SettingGroup>
</SettingSection>
