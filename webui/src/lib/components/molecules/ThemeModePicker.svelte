<script lang="ts">
	// Header theme switcher: the kit's compact icon-button + native <select>,
	// with an "Auto" entry ahead of the light and dark sections. Auto follows
	// the system scheme and its label recalls the two remembered themes, so the
	// user sees what day and night will paint before picking it.
	import { SelectButton } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { settings } from '$lib/settings.svelte';
	import { theme } from '$lib/theme.svelte';
	import { themeMode } from '$lib/themeMode.svelte';
	import { themePickerGroups, themePickerTitle } from './themePicker.logic';

	let { class: klass = '' }: { class?: string } = $props();

	const groups = $derived(
		themePickerGroups(theme.all, themeMode.pref, {
			auto: m.theme_auto_label(),
			light: m.theme_group_light(),
			dark: m.theme_group_dark()
		})
	);
	const title = $derived(
		themePickerTitle(theme.all, themeMode.pref, themeMode.resolved, m.theme_auto_label())
	);
</script>

<SelectButton
	data-tsu="ThemePicker"
	class={klass}
	glyph={themeMode.pref.mode === 'auto' ? '◐' : theme.icon}
	active={themeMode.pref.mode === 'auto'}
	label={m.settings_theme_label()}
	{title}
	value={themeMode.value}
	{groups}
	onchange={(v) => settings.setTheme(v)}
/>
