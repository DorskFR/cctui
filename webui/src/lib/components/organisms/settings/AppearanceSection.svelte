<script lang="ts">
	// Interface › Theme & look. Theme, text size, language and toast position
	// ride the settings blob and follow the user; look, motion and sound effects
	// live in the kit's own browser storage, which the section says once.
	import {
		FX_FAMILIES,
		fx,
		LookPicker,
		SegmentedControl,
		Select,
		sound,
		Switch,
		Text
	} from '@dorsk/tsumikit';
	import type { SegmentOption } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import MotionCustomize from './MotionCustomize.svelte';
	import SoundCustomize from './SoundCustomize.svelte';
	import { settings, type ToastPosition } from '$lib/settings.svelte';
	import { LOCALE_LABELS, LOCALES, type Locale } from '$lib/locale.svelte';
	import { theme } from '$lib/theme.svelte';
	import { themePickerGroups } from '$lib/components/molecules/themePicker.logic';
	import { fontScale, SCALE_LEVELS } from '$lib/fontscale.svelte';
	import { m } from '$lib/paraglide/messages';

	const LOOKS = ['default', 'depth', 'tactile'] as const;

	const scaleOptions: SegmentOption[] = SCALE_LEVELS.map((l) => ({ value: l.id, label: l.label }));
	function scaleOf(id: string): number {
		return SCALE_LEVELS.find((l) => l.id === id)?.value ?? 1;
	}
	const themeGroups = $derived(
		themePickerGroups(theme.all, theme.pref, {
			auto: m.theme_auto_label(),
			light: m.theme_group_light(),
			dark: m.theme_group_dark()
		})
	);

	const motionOn = $derived(FX_FAMILIES.some((f) => fx.settings[f] !== 'none'));
	function setMotion(on: boolean) {
		if (on) fx.reset();
		else for (const f of FX_FAMILIES) fx.set(f, 'none');
	}
</script>

{#snippet scaleGlyph(o: SegmentOption)}
	<Text style="font-size: {Math.min(scaleOf(o.value), 1.5)}em; line-height: 1" aria-hidden="true">A</Text>
{/snippet}

<SettingSection id="theme" icon="palette" title={m.set_theme_title()} description={m.set_theme_desc()}>
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

	<SettingGroup title={m.set_this_device()}>
		<SettingRow label={m.settings_look_label()} help={m.set_look_help()} selfLabelled>
			<div data-testid="appearance-look">
				<LookPicker looks={LOOKS} experimental={false} size="sm" control label={m.settings_look_label()} />
			</div>
		</SettingRow>
		<SettingRow label={m.settings_motion_label()} help={m.set_motion_help()} selfLabelled>
			<div class="pair" data-testid="appearance-motion">
				<MotionCustomize disabled={!motionOn} />
				<Switch bind:checked={() => motionOn, setMotion} label={m.settings_motion_label()} />
			</div>
		</SettingRow>
		<SettingRow label={m.settings_sound_switch()} help={m.set_sound_help()} selfLabelled>
			<div class="pair" data-testid="appearance-sound">
				<SoundCustomize disabled={!sound.enabled} />
				<Switch
					bind:checked={() => sound.enabled, (v) => sound.setEnabled(v)}
					label={m.settings_sound_switch()}
				/>
			</div>
		</SettingRow>
	</SettingGroup>
</SettingSection>

<style>
	.pair {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
	}
</style>
