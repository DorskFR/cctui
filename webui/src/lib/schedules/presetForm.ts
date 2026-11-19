import type { LaunchPreset } from '@bindings/LaunchPreset';
import type { LaunchPresetRequest } from '@bindings/LaunchPresetRequest';
import type { Overlap } from '@bindings/Overlap';
import type { PermissionMode } from '@bindings/PermissionMode';
import { NO_ACCOUNT, POOL_PREFIX } from '#lib/components/organisms/spawn/options.ts';
import { cronFromRepeat, cronProblem, parseTime, repeatFromCron, type CronProblem, type Repeat } from './cron';

export interface PresetForm {
	name: string;
	prompt: string;
	machine_id: string;
	working_dir: string;
	adapter_id: string;
	model: string;
	permission_mode: PermissionMode | null;
	/** '' Auto · NO_ACCOUNT · POOL_PREFIX + pool id · account id */
	account: string;
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
}

export function accountPickOf(p: Pick<LaunchPreset, 'account_id' | 'pool_id' | 'no_account'>): string {
	if (p.no_account) return NO_ACCOUNT;
	if (p.pool_id) return `${POOL_PREFIX}${p.pool_id}`;
	return p.account_id ?? '';
}

export function accountFields(pick: string): Pick<LaunchPresetRequest, 'account_id' | 'pool_id' | 'no_account'> {
	if (pick === NO_ACCOUNT) return { account_id: null, pool_id: null, no_account: true };
	if (pick.startsWith(POOL_PREFIX)) return { account_id: null, pool_id: pick.slice(POOL_PREFIX.length), no_account: false };
	return { account_id: pick || null, pool_id: null, no_account: false };
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
		notify_url: preset.notify_url
	};
}

export const withEnabled = (preset: LaunchPreset, enabled: boolean): LaunchPresetRequest => ({
	...requestFromPreset(preset),
	enabled
});

export function emptyForm(defaults: { machine_id?: string; adapter_id?: string; timezone: string }): PresetForm {
	return {
		name: '',
		prompt: '',
		machine_id: defaults.machine_id ?? '',
		working_dir: '~',
		adapter_id: defaults.adapter_id ?? '',
		model: '',
		permission_mode: null,
		account: '',
		repeat: repeatFromCron(null),
		timezone: defaults.timezone,
		overlap: 'skip',
		enabled: true
	};
}

export function formFromPreset(preset: LaunchPreset, fallbackTimezone: string): PresetForm {
	return {
		name: preset.name,
		prompt: preset.prompt,
		machine_id: preset.machine_id ?? '',
		working_dir: preset.working_dir ?? '',
		adapter_id: preset.adapter_id ?? '',
		model: preset.model ?? '',
		permission_mode: preset.permission_mode,
		account: accountPickOf(preset),
		repeat: repeatFromCron(preset.cron),
		timezone: preset.cron === null ? fallbackTimezone : preset.timezone || 'UTC',
		overlap: preset.overlap,
		enabled: preset.enabled
	};
}

const isAbsolute = (dir: string): boolean => dir.startsWith('/') || dir === '~' || dir.startsWith('~/');

export const isGuarded = (mode: PermissionMode | null): boolean => mode === 'ask' || mode === 'auto';

export function formProblems(
	form: PresetForm,
	opts: { schedule: boolean; dispatcher?: boolean; external?: boolean }
): PresetFormProblems {
	const out: PresetFormProblems = {};
	if (!form.name.trim()) out.name = 'required';
	if (!form.prompt.trim()) out.prompt = 'required';
	if (!opts.dispatcher) {
		if (!form.machine_id) out.machine = 'required';
		const dir = form.working_dir.trim();
		if (!dir) out.working_dir = 'required';
		else if (!isAbsolute(dir)) out.working_dir = 'absolute';
		if (!form.adapter_id) out.adapter = 'required';
		if (opts.external && !isGuarded(form.permission_mode)) out.permission_mode = 'guarded';
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
	notify_url: null
} satisfies Partial<LaunchPresetRequest>;

export function requestFromForm(
	form: PresetForm,
	opts: { schedule: boolean; base?: LaunchPreset | null; external?: boolean }
): LaunchPresetRequest {
	const kept = opts.base ? requestFromPreset(opts.base) : null;
	const target = isDispatcherTarget(opts.base)
		? {}
		: {
				machine_id: form.machine_id || null,
				working_dir: form.working_dir.trim() || null,
				adapter_id: form.adapter_id || null,
				model: form.model.trim() || null,
				permission_mode: form.permission_mode,
				...accountFields(form.account)
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
		name: form.name.trim(),
		prompt: form.prompt,
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
