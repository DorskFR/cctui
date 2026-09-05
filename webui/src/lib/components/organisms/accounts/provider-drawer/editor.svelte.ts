import type { AccountModel, AccountProvider, UpdateProvider, UsageNotices } from '$lib/queries';
import { isStaticCredential } from '$lib/providers';
import {
	aliasObject,
	buildRateLimits,
	buildSoftLimits,
	buildUsageNotices,
	fwModelList,
	modelList,
	type SoftEdit
} from '../account-editor.logic';
import { diffCount, settingsSlice, softFlat, type PageId } from './pages.logic';
import { knobEnvNames, knobKeyNames, type KnobGroup } from './knobs.logic';

const seedSoft = (p: AccountProvider): Record<string, SoftEdit> => {
	const out: Record<string, SoftEdit> = {};
	for (const [key, v] of Object.entries(p.soft_limits ?? {})) {
		out[key] = {
			cap: v.cap_pct ?? null,
			capUsd: v.cap_usd ?? null,
			bypass: v.bypass_minutes ?? null
		};
	}
	return out;
};

const indexed = (list: AccountModel[]) =>
	Object.fromEntries(list.map((mo, i) => [String(i), mo])) as Record<string, unknown>;

/** `orig` is the load-time snapshot the per-page change counts are measured
 *  against; it must not follow later refetches of the provider row. */
export class ProviderEdit {
	readonly provider: AccountProvider;
	readonly isFireworks: boolean;
	readonly isCompatible: boolean;
	readonly isAnthropic: boolean;

	page = $state<PageId>('limits');
	aliasRows = $state<{ alias: string; model: string }[]>([]);
	soft = $state<Record<string, SoftEdit>>({});
	rate = $state<{ rpm: number | null; tpm: number | null }>({ rpm: null, tpm: null });
	notices = $state<UsageNotices>({ enabled: false, step_pct: 10 });
	settings = $state<Record<string, unknown>>({});
	providerSettings = $state<Record<string, unknown>>({});
	models = $state<AccountModel[]>([]);
	baseUrl = $state('');
	credential = $state('');
	authScheme = $state<'bearer' | 'api_key' | 'keep'>('keep');
	pinned = $state<boolean | null>(null);

	private readonly orig;

	constructor(p: AccountProvider) {
		this.provider = p;
		this.isFireworks = p.provider === 'fireworks';
		this.isCompatible = isStaticCredential(p.provider);
		this.isAnthropic = p.family === 'anthropic';
		this.aliasRows = Object.entries(p.model_aliases ?? {}).map(([alias, model]) => ({
			alias,
			model
		}));
		this.soft = seedSoft(p);
		this.rate = { rpm: p.rate_limits?.rpm ?? null, tpm: p.rate_limits?.tpm ?? null };
		this.notices = {
			enabled: p.usage_notices?.enabled ?? false,
			step_pct: p.usage_notices?.step_pct ?? 10
		};
		this.settings = { ...(p.settings_json ?? {}) };
		this.providerSettings = { ...(p.provider_settings ?? {}) };
		this.models = (p.models ?? []).map((mo) => ({ ...mo }));
		this.orig = {
			aliases: { ...(p.model_aliases ?? {}) },
			soft: softFlat(seedSoft(p)),
			rate: { ...this.rate },
			notices: { ...this.notices },
			settings: { ...this.settings },
			providerSettings: { ...this.providerSettings },
			models: indexed(this.models)
		};
	}

	seedWindows(keys: string[]) {
		for (const key of keys) {
			if (!(key in this.soft)) this.soft[key] = { cap: null, capUsd: null, bypass: null };
		}
	}

	knobDiff(groups: KnobGroup[]): number {
		const knobs = groups.flatMap((g) => g.knobs);
		if (!knobs.length) return 0;
		const keys = knobKeyNames(knobs);
		const envs = knobEnvNames(knobs);
		return diffCount(
			settingsSlice(this.settings, keys, envs),
			settingsSlice(this.orig.settings, keys, envs)
		);
	}

	changes(groups: KnobGroup[]): number {
		const knobs = this.knobDiff(groups);
		switch (this.page) {
			case 'aliases':
				return diffCount(aliasObject(this.aliasRows), this.orig.aliases);
			case 'limits':
				return (
					diffCount(softFlat(this.soft), this.orig.soft) +
					diffCount({ ...this.rate }, { ...this.orig.rate })
				);
			case 'models':
				return diffCount(indexed(this.models), this.orig.models);
			case 'gateway':
				return (
					knobs +
					diffCount({ ...this.notices }, { ...this.orig.notices }) +
					(this.isFireworks ? diffCount(this.providerSettings, this.orig.providerSettings) : 0)
				);
			case 'ui':
				return (
					knobs +
					(this.isAnthropic ? diffCount(this.providerSettings, this.orig.providerSettings) : 0)
				);
			case 'advanced':
				return (
					knobs +
					(this.baseUrl.trim() ? 1 : 0) +
					(this.credential.trim() ? 1 : 0) +
					(this.authScheme === 'keep' ? 0 : 1)
				);
			default:
				return knobs;
		}
	}

	body(): UpdateProvider {
		const out: UpdateProvider = {
			model_aliases: aliasObject(this.aliasRows),
			soft_limits: buildSoftLimits(this.soft),
			rate_limits: buildRateLimits(this.rate),
			usage_notices: buildUsageNotices(this.notices),
			...(this.isAnthropic
				? { settings_json: this.settings, provider_settings: this.providerSettings }
				: {})
		};
		if (this.isFireworks) {
			out.models = fwModelList(this.models);
			out.provider_settings = this.providerSettings;
		} else if (this.isCompatible) {
			out.models = modelList(this.models);
		}
		if (this.isFireworks || this.isCompatible) {
			if (this.baseUrl.trim()) out.base_url = this.baseUrl.trim();
			if (this.credential.trim()) out.access_token = this.credential.trim();
			if (this.authScheme !== 'keep') out.auth_scheme = this.authScheme;
		}
		return out;
	}
}
