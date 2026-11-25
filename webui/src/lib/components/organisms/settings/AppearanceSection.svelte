<script lang="ts">
	// Settings › Interface › Theme & look. Look, motion and sound live in the
	// kit's own browser storage (this device only); the other blob-backed
	// wrappers on `settings` drive the runtime theme/fontScale singletons AND
	// persist, so this section and the header pickers share one surface.
	import {
		LookPicker,
		MotionSettings,
		SegmentedControl,
		Select,
		SoundSettings,
		Text
	} from '@dorsk/tsumikit';
	import type { SegmentOption } from '@dorsk/tsumikit';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import SettingSection from '#lib/components/molecules/SettingSection.svelte';
	import { settings, type ToastPosition } from '#lib/settings.svelte.ts';
	import { LOCALE_LABELS, LOCALES, type Locale } from '#lib/locale.svelte.ts';
	import { theme } from '#lib/theme.svelte.ts';
	import { themePickerGroups } from '#lib/components/molecules/themePicker.logic.ts';
	import { fontScale, SCALE_LEVELS } from '#lib/fontscale.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';

	const LOOKS = ['default', 'depth', 'tactile'] as const;

	const scaleOptions: SegmentOption[] = SCALE_LEVELS.map((l) => ({ value: l.id, label: l.label }));
	function scaleOf(id: string): number {
		return SCALE_LEVELS.find((l) => l.id === id)?.value ?? 1;
	}
	// Auto first (its label recalls the remembered light and dark themes), then
	// the light and dark sections. Same groups as the header picker.
	const themeGroups = $derived(
		themePickerGroups(theme.all, theme.pref, {
			auto: m.theme_auto_label(),
			light: m.theme_group_light(),
			dark: m.theme_group_dark()
		})
	);
</script>

{#snippet scaleGlyph(o: SegmentOption)}
	<Text style="font-size: {Math.min(scaleOf(o.value), 1.5)}em; line-height: 1" aria-hidden="true">A</Text>
{/snippet}

{#snippet description()}
	{m.settings_appearance_desc()} {m.settings_appearance_device_note()}
{/snippet}

<SettingSection id="theme" icon="◐" title={m.settings_sec_theme()} descriptionSlot={description}>
	<SettingGroup>
		<SettingRow label={m.settings_theme_label()} help={m.settings_theme_help()}>
			<Select
				data-journey="theme"
				value={theme.choice}
				style="width:100%"
				onchange={(e) => settings.setTheme((e.currentTarget as HTMLSelectElement).value)}
			>
				{#each themeGroups as g (g.label)}
					<optgroup label={g.label}>
						{#each g.options as o (o.value)}
							<option value={o.value}>{o.label}</option>
						{/each}
					</optgroup>
				{/each}
			</Select>
		</SettingRow>
		<SettingRow label={m.settings_look_label()} help={m.settings_look_help()} selfLabelled>
			<div data-testid="appearance-look">
				<LookPicker looks={LOOKS} experimental={false} size="sm" control label={m.settings_look_label()} />
			</div>
		</SettingRow>
		<SettingRow label={m.settings_motion_label()} help={m.settings_motion_help()} selfLabelled>
			<div data-testid="appearance-motion">
				<MotionSettings
					size="sm"
					control
					label={m.settings_motion_label()}
					resetLabel={m.settings_motion_reset()}
					reducedMotionHint={m.settings_motion_reduced_hint()}
				/>
			</div>
		</SettingRow>
		<SettingRow label={m.settings_sound_label()} help={m.settings_sound_help()} selfLabelled>
			<div data-testid="appearance-sound">
				<SoundSettings
					size="sm"
					control
					label={m.settings_sound_label()}
					switchLabel={m.settings_sound_switch()}
					volumeLabel={m.settings_sound_volume()}
					previewLabel={m.settings_sound_preview()}
					offHint={m.settings_sound_off_hint()}
					reducedMotionHint={m.settings_sound_reduced_hint()}
				/>
			</div>
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
				data-journey="language"
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
	</SettingGroup>
</SettingSection>
