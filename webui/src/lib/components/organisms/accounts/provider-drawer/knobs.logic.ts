import type { EnvVar } from '@bindings/EnvVar';
import type { Preset } from '@bindings/Preset';
import type { SettingsCatalogResponse } from '@bindings/SettingsCatalogResponse';
import { groupPage, type PageId } from './pages.logic';

export type KnobControl = 'tristate' | 'toggle' | 'enum' | 'number' | 'string';

export interface Knob {
	id: string;
	label: string;
	sub: string;
	care: boolean;
	control: KnobControl;
	loc: 'setting' | 'env';
	name: string;
	values?: string[];
	/** Env twin of a settings key: written when the value is outside the key's
	 *  own enum (mirrors the daemon's effortLevel / CLAUDE_CODE_EFFORT_LEVEL split). */
	envName?: string;
	settingsValues?: string[];
	note: string;
}

export interface KnobGroup {
	title: string;
	page: PageId;
	knobs: Knob[];
}

function boolKnob(name: string, label: string, care: boolean, note: string, twin?: EnvVar): Knob {
	return {
		id: `s:${name}`,
		label,
		sub: twin ? `${name} · ${twin.name}` : name,
		care,
		control: 'tristate',
		loc: 'setting',
		name,
		note
	};
}

function envKnob(e: EnvVar, enumOf: (name: string) => string[] | undefined): Knob {
	const control: KnobControl =
		e.kind === 'flag'
			? 'toggle'
			: e.kind === 'number'
				? 'number'
				: e.kind === 'enum'
					? 'enum'
					: 'string';
	return {
		id: `e:${e.name}`,
		label: e.label ?? e.name,
		sub: e.settings_equiv ? `${e.name} · ${e.settings_equiv}` : e.name,
		care: e.tag === 'care',
		control,
		loc: e.settings_equiv ? 'setting' : 'env',
		name: e.settings_equiv ?? e.name,
		values: e.values ?? undefined,
		envName: e.settings_equiv ? e.name : undefined,
		settingsValues: e.settings_equiv ? enumOf(e.settings_equiv) : undefined,
		note: e.notes ?? ''
	};
}

/** Curated boolean settings groups first (each with its env twin merged in),
 *  then the remaining curated env vars by their own group. Exact aliases and
 *  vars already merged into a boolean row are dropped, so a knob renders once. */
export function knobGroups(catalog: SettingsCatalogResponse | undefined): KnobGroup[] {
	if (!catalog) return [];
	const keys = catalog.keys ?? [];
	const env: EnvVar[] = catalog.env ?? [];
	const boolKeys = keys.filter((k) => k.group !== null);
	const twins = new Map(
		env.filter((e) => e.settings_equiv).map((e) => [e.settings_equiv as string, e])
	);
	const enumOf = (name: string) => {
		const k = keys.find((x) => x.name === name);
		return k?.enum ? k.enum.split(',').map((v) => v.trim()) : undefined;
	};

	const out: KnobGroup[] = [];
	for (const title of [...new Set(boolKeys.map((k) => k.group as string))]) {
		out.push({
			title,
			page: groupPage(title),
			knobs: boolKeys
				.filter((k) => k.group === title)
				.map((k) =>
					boolKnob(k.name, k.label ?? k.name, k.tag === 'care', k.notes ?? '', twins.get(k.name))
				)
		});
	}
	const boolNames = new Set(boolKeys.map((k) => k.name));
	const rest = env.filter(
		(e) => !e.env_alias_of && !(e.settings_equiv && boolNames.has(e.settings_equiv))
	);
	for (const title of [...new Set(rest.map((e) => e.group))]) {
		out.push({
			title,
			page: groupPage(title),
			knobs: rest.filter((e) => e.group === title).map((e) => envKnob(e, enumOf))
		});
	}
	return out;
}

export function envObj(settings: Record<string, unknown>): Record<string, string> {
	const e = settings.env;
	return e && typeof e === 'object' && !Array.isArray(e) ? (e as Record<string, string>) : {};
}

export function setSetting(
	settings: Record<string, unknown>,
	name: string,
	v: unknown
): Record<string, unknown> {
	const next = { ...settings };
	if (v === undefined || v === '') delete next[name];
	else next[name] = v;
	return next;
}

export function setEnv(
	settings: Record<string, unknown>,
	name: string,
	v: string
): Record<string, unknown> {
	const env = { ...envObj(settings) };
	if (v === '') delete env[name];
	else env[name] = v;
	const next = { ...settings };
	if (Object.keys(env).length) next.env = env;
	else delete next.env;
	return next;
}

export function getKnob(settings: Record<string, unknown>, k: Knob): string {
	if (k.control === 'tristate') {
		const v = settings[k.name];
		return v === true ? 'true' : v === false ? 'false' : '';
	}
	if (k.loc === 'setting') {
		const v = settings[k.name];
		if (typeof v === 'string') return v;
		return k.envName ? (envObj(settings)[k.envName] ?? '') : '';
	}
	return envObj(settings)[k.name] ?? '';
}

export function setKnob(
	settings: Record<string, unknown>,
	k: Knob,
	v: string
): Record<string, unknown> {
	if (k.control === 'tristate') {
		return setSetting(settings, k.name, v === 'true' ? true : v === 'false' ? false : undefined);
	}
	if (k.loc !== 'setting') return setEnv(settings, k.name, v);
	if (k.envName && k.settingsValues && v !== '' && !k.settingsValues.includes(v)) {
		return setEnv(setSetting(settings, k.name, undefined), k.envName, v);
	}
	const next = setSetting(settings, k.name, v);
	return k.envName ? setEnv(next, k.envName, '') : next;
}

export function overriddenCount(settings: Record<string, unknown>, knobs: Knob[]): number {
	return knobs.filter((k) => getKnob(settings, k) !== '').length;
}

export function clearKnobs(
	settings: Record<string, unknown>,
	knobs: Knob[]
): Record<string, unknown> {
	let next = settings;
	for (const k of knobs) next = setKnob(next, k, '');
	return next;
}

export function applyPreset(
	settings: Record<string, unknown>,
	preset: Preset | undefined,
	knobs: Knob[]
): Record<string, unknown> {
	if (!preset) return settings;
	const keys = new Set(knobKeyNames(knobs));
	const envs = new Set(knobEnvNames(knobs));
	let next = settings;
	for (const [name, v] of Object.entries(preset.settings)) {
		if (keys.has(name)) next = setSetting(next, name, v);
	}
	for (const [name, v] of Object.entries(preset.env)) {
		if (envs.has(name)) next = setEnv(next, name, v);
	}
	return next;
}

export function knobKeyNames(knobs: Knob[]): string[] {
	return knobs.filter((k) => k.loc === 'setting').map((k) => k.name);
}

export function knobEnvNames(knobs: Knob[]): string[] {
	return knobs.map((k) => (k.loc === 'setting' ? k.envName : k.name)).filter((n): n is string => !!n);
}
