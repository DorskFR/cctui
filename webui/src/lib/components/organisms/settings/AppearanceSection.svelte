<script lang="ts">
	// Settings › Appearance: theme, text size, interface language, toast
	// position. The blob-backed wrappers on `settings` drive the runtime
	// theme/fontScale singletons AND persist, so this section and the header
	// pickers share one round-tripping surface.
	import { SegmentedControl, Select, Switch, Text } from '@dorsk/tsumikit';
	import type { SegmentOption } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import { settings, type ToastPosition } from '$lib/settings.svelte';
	import { LOCALE_LABELS, LOCALES, type Locale } from '$lib/locale.svelte';
	import { AUTO, theme, THEMES } from '$lib/theme.svelte';
	import { fontScale, SCALE_LEVELS } from '$lib/fontscale.svelte';
	import { m } from '$lib/paraglide/messages';

	const scaleOptions: SegmentOption[] = SCALE_LEVELS.map((l) => ({ value: l.id, label: l.label }));
	function scaleOf(id: string): number {
		return SCALE_LEVELS.find((l) => l.id === id)?.value ?? 1;
	}
</script>

{#snippet scaleGlyph(o: SegmentOption)}
	<Text style="font-size: {Math.min(scaleOf(o.value), 1.5)}em; line-height: 1" aria-hidden="true">A</Text>
{/snippet}

<SettingSection
	id="appearance"
	icon="◐"
	title={m.settings_nav_appearance()}
	description={m.settings_appearance_desc()}
>
	<SettingGroup>
		<SettingRow label={m.settings_theme_label()} help={m.settings_theme_help()}>
			<Select
				value={theme.current}
				style="width:100%"
				onchange={(e) => settings.setTheme((e.currentTarget as HTMLSelectElement).value)}
			>
				<option value={AUTO.id}>{AUTO.icon} {m.nav_theme_auto()}</option>
				{#each THEMES as t (t.id)}
					<option value={t.id}>{t.icon} {t.label}</option>
				{/each}
			</Select>
		</SettingRow>
		<SettingRow label={m.settings_font_size_label()} selfLabelled>
			<SegmentedControl
				options={scaleOptions}
				label={m.settings_font_size_label()}
				control
				option={scaleGlyph}
				bind:value={() => fontScale.levelId, (v) => settings.setFontScaleLevel(v ?? 'normal')}
			/>
		</SettingRow>
		<SettingRow
			label={m.settings_interface_language_label()}
			help={m.settings_interface_language_help()}
		>
			<Select
				value={settings.locale ?? 'auto'}
				style="width:100%"
				onchange={(e) => {
					const v = (e.currentTarget as HTMLSelectElement).value;
					settings.setLocale(v === 'auto' ? null : (v as Locale));
				}}
			>
				<option value="auto">{m.settings_language_auto()}</option>
				{#each LOCALES as l (l)}
					<option value={l}>{LOCALE_LABELS[l]}</option>
				{/each}
			</Select>
		</SettingRow>
		<SettingRow label={m.settings_toast_position_label()} help={m.settings_toast_position_help()}>
			<Select
				value={settings.toastPosition}
				style="width:100%"
				onchange={(e) =>
					settings.setToastPosition((e.currentTarget as HTMLSelectElement).value as ToastPosition)}
			>
				<option value="center">{m.settings_toast_position_center()}</option>
				<option value="left">{m.settings_toast_position_left()}</option>
				<option value="right">{m.settings_toast_position_right()}</option>
			</Select>
		</SettingRow>
		<SettingRow label={m.usage_battery_setting_label()} help={m.usage_battery_setting_help()}>
			<Switch
				checked={settings.usageBatteries}
				label={m.usage_battery_setting_label()}
				onclick={() => settings.setUsageBatteries(!settings.usageBatteries)}
			/>
		</SettingRow>
	</SettingGroup>
</SettingSection>
