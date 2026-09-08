import { AUTO_THEME, type ThemePreference } from '$lib/themeMode';

/** The subset of a kit theme definition the pickers need. */
export interface ThemeOptionDef {
	id: string;
	label: string;
	mode: 'light' | 'dark';
	icon?: string;
}

export interface PickerOption {
	value: string;
	label: string;
}
export interface PickerGroup {
	label: string;
	options: PickerOption[];
}

const FALLBACK_ICON = '◈';

export const themeLabel = (all: readonly ThemeOptionDef[], id: string): string =>
	all.find((t) => t.id === id)?.label ?? id;

const themeIcon = (all: readonly ThemeOptionDef[], id: string): string =>
	all.find((t) => t.id === id)?.icon ?? FALLBACK_ICON;

/** "Auto · ☀ Sepia / ☾ Mocha": the auto entry recalls both remembered themes. */
export function autoLabel(all: readonly ThemeOptionDef[], pref: ThemePreference, auto: string): string {
	return `${auto} · ${themeIcon(all, pref.light)} ${themeLabel(all, pref.light)} / ${themeIcon(all, pref.dark)} ${themeLabel(all, pref.dark)}`;
}

/** Native <optgroup> sections for a theme picker: Auto first, then light, then dark. */
export function themePickerGroups(
	all: readonly ThemeOptionDef[],
	pref: ThemePreference,
	labels: { auto: string; light: string; dark: string }
): PickerGroup[] {
	const toOption = (t: ThemeOptionDef): PickerOption => ({
		value: t.id,
		label: `${t.icon ?? FALLBACK_ICON}  ${t.label}`
	});
	return [
		{ label: labels.auto, options: [{ value: AUTO_THEME, label: `◐  ${autoLabel(all, pref, labels.auto)}` }] },
		{ label: labels.light, options: all.filter((t) => t.mode === 'light').map(toOption) },
		{ label: labels.dark, options: all.filter((t) => t.mode === 'dark').map(toOption) }
	];
}

/** Tooltip for the header button: the painted theme, prefixed by Auto when it
 *  came from the system. */
export function themePickerTitle(
	all: readonly ThemeOptionDef[],
	pref: ThemePreference,
	resolved: string,
	auto: string
): string {
	const name = themeLabel(all, resolved);
	return pref.mode === 'auto' ? `${auto} · ${name}` : name;
}
