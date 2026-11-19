import type { LaunchPreset } from '@bindings/LaunchPreset';
import type { LaunchPresetRequest } from '@bindings/LaunchPresetRequest';
import type { Overlap } from '@bindings/Overlap';
import type { PermissionMode } from '@bindings/PermissionMode';
import type { SpawnTemplate } from '@bindings/SpawnTemplate';
import { PERMISSION_MODES } from '#lib/domainTables.ts';
import { badEnvKeys, envEdit, storedEnvRows } from '#lib/components/organisms/spawn/envRows.ts';
import { EMPTY_SPEC, type ProfileSpecForm } from '#lib/components/organisms/spawn/profiles.ts';
import { blank } from '#lib/components/organisms/spawn/spawnSeed.ts';
import type { EnvRow, Form } from '#lib/components/organisms/spawn/types.ts';
import { cronFromRepeat, cronProblem, parseTime, repeatFromCron, type CronProblem, type Repeat } from './cron';

/** A schedule is a spawn template plus its repeat: `spawn` and `kit` are the
 * spawn form's own state, so the editor reuses the spawn form's fields. */
export interface PresetForm {
	/** Where, the name every run carries, the prompt, labels and context. */
	spawn: Form;
	/** Harness · account · model · effort · permission mode · service tier. */
	kit: ProfileSpecForm;
	env: EnvRow[];
	repeat: Repeat;
	timezone: string;
	overlap: Overlap;
	enabled: boolean;
}

export interface PresetFormProblems {
	name?: 'required';
	prompt?: 'required';
	machine?: 'required';
	working_dir?: 'required' | 'absolute';
	adapter?: 'required';
	permission_mode?: 'guarded';
	time?: 'invalid';
	weekdays?: 'required';
	cron?: CronProblem;
	env?: 'invalid';
}

export const isSchedule = (preset: Pick<LaunchPreset, 'cron'>): boolean => preset.cron !== null;

export const isDispatcherTarget = (preset: Pick<LaunchPreset, 'dispatcher_id'> | null | undefined): boolean =>
	!!preset?.dispatcher_id;

export function requestFromPreset(preset: LaunchPreset): LaunchPresetRequest {
	return {
		name: preset.name,
		prompt: preset.prompt,
		machine_id: preset.machine_id,
		working_dir: preset.working_dir,
		adapter_id: preset.adapter_id,
		model: preset.model,
		permission_mode: preset.permission_mode,
		account_id: preset.account_id,
		pool_id: preset.pool_id,
		no_account: preset.no_account,
		dispatcher_id: preset.dispatcher_id,
		profile: preset.profile,
		external_sources: preset.external_sources,
		fresh_secs: preset.fresh_secs,
		on_stale: preset.on_stale,
		max_batch: preset.max_batch,
		cron: preset.cron,
		timezone: preset.timezone,
		enabled: preset.enabled,
		overlap: preset.overlap,
		then_preset_id: preset.then_preset_id,
		notify_url: preset.notify_url,
		env_keys: null,
		env: null,
		template: preset.template ?? null
	};
}

export const withEnabled = (preset: LaunchPreset, enabled: boolean): LaunchPresetRequest => ({
	...requestFromPreset(preset),
	enabled
});

export function emptyForm(defaults: { machine_id?: string; adapter_id?: string; timezone: string }): PresetForm {
	const harness = defaults.adapter_id || 'claude-code';
	return {
		spawn: { ...blank, machine_id: defaults.machine_id ?? '', working_dir: '~', adapter_id: harness, labels: [], context_items: [] },
		kit: { ...EMPTY_SPEC, harness },
		env: [],
		repeat: repeatFromCron(null),
		timezone: defaults.timezone,
		overlap: 'skip',
		enabled: true
	};
}

export function formFromPreset(preset: LaunchPreset, fallbackTimezone: string): PresetForm {
	const t: SpawnTemplate = preset.template ?? {};
	const harness = preset.adapter_id || 'claude-code';
	return {
		spawn: {
			...blank,
			machine_id: preset.machine_id ?? '',
			working_dir: preset.working_dir ?? '',
			adapter_id: harness,
			name: preset.name,
			prompt: preset.prompt,
			labels: [...(t.label_ids ?? [])],
			context_items: [...(t.context?.items ?? [])],
			context_auto: t.context?.auto ?? false
		},
		kit: {
			harness,
			account_id: preset.account_id,
			pool_id: preset.pool_id,
			no_account: preset.no_account,
			model_alias: preset.model,
			effort: t.effort ?? null,
			permission_mode: preset.permission_mode,
			service_tier: t.service_tier ?? null
		},
		env: storedEnvRows(preset.env_keys ?? []),
		repeat: repeatFromCron(preset.cron),
		timezone: preset.cron === null ? fallbackTimezone : preset.timezone || 'UTC',
		overlap: preset.overlap,
		enabled: preset.enabled
	};
}

const isAbsolute = (dir: string): boolean => dir.startsWith('/') || dir === '~' || dir.startsWith('~/');

export const isGuarded = (mode: string | null): boolean => mode === 'ask' || mode === 'auto';

export function formProblems(
	form: PresetForm,
	opts: { schedule: boolean; dispatcher?: boolean; external?: boolean }
): PresetFormProblems {
	const out: PresetFormProblems = {};
	if (!form.spawn.name.trim()) out.name = 'required';
	if (!form.spawn.prompt.trim()) out.prompt = 'required';
	if (badEnvKeys(form.env).length) out.env = 'invalid';
	if (!opts.dispatcher) {
		if (!form.spawn.machine_id) out.machine = 'required';
		const dir = form.spawn.working_dir.trim();
		if (!dir) out.working_dir = 'required';
		else if (!isAbsolute(dir)) out.working_dir = 'absolute';
		if (!form.kit.harness) out.adapter = 'required';
		if (opts.external && !isGuarded(form.kit.permission_mode)) out.permission_mode = 'guarded';
	}
	if (opts.schedule) {
		const { kind } = form.repeat;
		if (kind === 'custom') {
			const problem = cronProblem(form.repeat.custom);
			if (problem) out.cron = problem;
		} else if (kind !== 'hourly') {
			if (!parseTime(form.repeat.time)) out.time = 'invalid';
			if (kind === 'weekly' && form.repeat.weekdays.length === 0) out.weekdays = 'required';
		}
	}
	return out;
}

export const hasProblems = (problems: PresetFormProblems): boolean => Object.keys(problems).length > 0;

const DEFAULTS = {
	dispatcher_id: null,
	profile: null,
	external_sources: false,
	fresh_secs: null,
	on_stale: null,
	max_batch: null,
	cron: null,
	timezone: null,
	enabled: null,
	overlap: null,
	then_preset_id: null,
	notify_url: null,
	env_keys: null,
	env: null,
	template: null
} satisfies Partial<LaunchPresetRequest>;

const known = (mode: string | null): PermissionMode | null =>
	PERMISSION_MODES.includes(mode as PermissionMode) ? (mode as PermissionMode) : null;

/** What the spawn form's fields set on every run; profile, auto-archive and
 * the stored files are kept from the preset. */
export function templateFromForm(form: PresetForm, base?: LaunchPreset | null): SpawnTemplate {
	const kept: SpawnTemplate = base?.template ?? {};
	const { context_items: items, context_auto: auto } = form.spawn;
	return {
		...kept,
		effort: form.kit.effort?.trim() || null,
		service_tier: form.kit.service_tier?.trim() || null,
		label_ids: [...form.spawn.labels],
		context: items.length || auto ? { items: [...items], auto } : null,
		attachment_names: kept.attachment_names ?? []
	};
}

export function requestFromForm(
	form: PresetForm,
	opts: { schedule: boolean; base?: LaunchPreset | null; external?: boolean }
): LaunchPresetRequest {
	const kept = opts.base ? requestFromPreset(opts.base) : null;
	const { kit } = form;
	const target = isDispatcherTarget(opts.base)
		? {}
		: {
				machine_id: form.spawn.machine_id || null,
				working_dir: form.spawn.working_dir.trim() || null,
				adapter_id: kit.harness || null,
				model: kit.model_alias?.trim() || null,
				permission_mode: known(kit.permission_mode),
				account_id: kit.no_account || kit.pool_id ? null : kit.account_id,
				pool_id: kit.no_account ? null : kit.pool_id,
				no_account: kit.no_account,
				template: templateFromForm(form, opts.base)
			};
	const schedule = opts.schedule
		? {
				cron: cronFromRepeat(form.repeat),
				timezone: form.timezone || null,
				enabled: form.enabled,
				overlap: form.overlap
			}
		: {};
	return {
		machine_id: null,
		working_dir: null,
		adapter_id: null,
		model: null,
		permission_mode: null,
		account_id: null,
		pool_id: null,
		no_account: false,
		...DEFAULTS,
		...kept,
		name: form.spawn.name.trim(),
		prompt: form.spawn.prompt,
		...envEdit(form.env),
		...target,
		...schedule,
		...(opts.external ? { external_sources: true } : {})
	};
}

export function timezoneOptions(supported: readonly string[], current: string): string[] {
	const out = new Set<string>(['UTC', ...supported]);
	if (current) out.add(current);
	return [...out].sort((a, b) => (a === 'UTC' ? -1 : b === 'UTC' ? 1 : a.localeCompare(b)));
}

export function browserTimezone(): string {
	try {
		return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
	} catch {
		return 'UTC';
	}
}

export function supportedTimezones(): string[] {
	const intl = Intl as typeof Intl & { supportedValuesOf?: (key: string) => string[] };
	try {
		return intl.supportedValuesOf?.('timeZone') ?? [];
	} catch {
		return [];
	}
}
