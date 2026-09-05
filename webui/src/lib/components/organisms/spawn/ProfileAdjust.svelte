<script lang="ts">
	import type { SessionProfile } from '@bindings/SessionProfile';
	import type { AccountPoolView } from '@bindings/AccountPoolView';
	import type { AccountUsageEntry, OAuthAccount } from '$lib/queries';
	import { useCodexModels, useMergedCodexModels } from '$lib/queries';
	import { Button, IconButton, Input, SegmentedControl, Select, Text } from '@dorsk/tsumikit';
	import type { SelectOption } from '@dorsk/tsumikit';
	import AdapterIcon from '$lib/components/atoms/AdapterIcon.svelte';
	import EffortSlider from './EffortSlider.svelte';
	import { preferCatalog } from '$lib/harnessModels';
	import {
		accountBacksAdapter,
		adapterLabel,
		allAdapters,
		claudeEfforts,
		claudeModels,
		codexEffortsFor,
		codexModelsFor,
		isCompatibleProvider,
		modes,
		NO_ACCOUNT,
		POOL_PREFIX,
		providerForAdapter,
		withAliasTargets
	} from './options';
	import { accountById, accountUsedPct, specChanges, specOf, type ProfileSpec } from './profiles';
	import { m } from '$lib/paraglide/messages';

	let {
		profile,
		initial,
		accounts,
		pools = [],
		usage,
		machineId,
		busy = false,
		onuseonce,
		onsave,
		ondelete
	}: {
		profile: SessionProfile;
		initial: ProfileSpec;
		accounts: OAuthAccount[];
		pools?: AccountPoolView[];
		usage: AccountUsageEntry[];
		machineId: string;
		busy?: boolean;
		onuseonce: (spec: ProfileSpec) => void;
		onsave: (name: string, spec: ProfileSpec) => void;
		ondelete: () => void;
	} = $props();

	// svelte-ignore state_referenced_locally
	let draft = $state<ProfileSpec>({ ...initial });
	// svelte-ignore state_referenced_locally
	let name = $state(profile.name);

	const saved = $derived(specOf(profile));
	const changes = $derived(specChanges(saved, draft) + (name.trim() !== profile.name ? 1 : 0));

	const account = $derived(accountById(accounts, draft.account_id));
	const provider = $derived(providerForAdapter(account, draft.harness));
	const usesAccountModels = $derived(!!provider && isCompatibleProvider(provider.provider));

	// One picker value space: '' Auto · the no-account sentinel · pool ids
	// behind POOL_PREFIX · account ids.
	const accountPick = $derived(
		draft.no_account
			? NO_ACCOUNT
			: draft.pool_id
				? `${POOL_PREFIX}${draft.pool_id}`
				: (draft.account_id ?? '')
	);
	function setAccountPick(v: string) {
		draft.no_account = v === NO_ACCOUNT;
		draft.pool_id = v.startsWith(POOL_PREFIX) ? v.slice(POOL_PREFIX.length) : null;
		draft.account_id = v && v !== NO_ACCOUNT && !v.startsWith(POOL_PREFIX) ? v : null;
	}
	const accountOptions = $derived<SelectOption[]>([
		{ value: '', label: m.spawn_account_auto() },
		{ value: NO_ACCOUNT, label: m.spawn_account_none() },
		...pools.map((p) => ({
			value: `${POOL_PREFIX}${p.id}`,
			label: p.name,
			hint: m.spawn_account_pool_group()
		})),
		...accounts
			.filter((a) => accountBacksAdapter(a, draft.harness))
			.map((a) => {
				const pct = accountUsedPct(usage, a.id);
				return {
					value: a.id,
					label: a.name,
					emoji: a.emoji ?? undefined,
					hint: pct === null ? undefined : `${pct}%`
				};
			})
	]);

	const machineCodex = useCodexModels(() => (draft.harness === 'codex' ? machineId : ''));
	const mergedCodex = useMergedCodexModels(() => draft.harness === 'codex');
	const codexCatalog = $derived(preferCatalog(machineCodex.data, mergedCodex.data));
	const modelOptions = $derived.by<SelectOption[]>(() => {
		const list = usesAccountModels
			? (provider?.models ?? []).map((x) => ({ v: x.model, label: x.label }))
			: draft.harness === 'codex'
				? codexModelsFor(codexCatalog)
				: withAliasTargets(claudeModels, provider?.model_aliases);
		const out = list.map((o) => ({ value: o.v, label: o.v ? o.label : m.spawn_model_default() }));
		if (!out.some((o) => o.value === '')) out.unshift({ value: '', label: m.spawn_model_default() });
		const current = draft.model_alias ?? '';
		if (current && !out.some((o) => o.value === current)) out.push({ value: current, label: current });
		return out;
	});
	const efforts = $derived(
		draft.harness === 'codex' ? codexEffortsFor(codexCatalog, draft.model_alias ?? '') : claudeEfforts
	);

	function pickHarness(harness: string) {
		if (harness === draft.harness) return;
		draft.harness = harness;
		draft.model_alias = null;
		draft.effort = null;
		if (account && !accountBacksAdapter(account, harness)) draft.account_id = null;
	}

	const modeOptions = $derived([
		{ value: '', label: m.spawn_mode_default_label() },
		...modes.map((md) => ({ value: md.v, label: md.label }))
	]);
</script>

<div class="panel">
	<div class="name-row">
		<Input
			size="sm"
			bind:value={name}
			aria-label={m.spawn_profile_name_aria()}
			placeholder={m.spawn_profile_name_aria()}
		/>
		<IconButton icon="trash" label={m.spawn_profile_delete()} inline hoverDanger onclick={ondelete} />
	</div>

	<div class="harness" role="radiogroup" aria-label={m.spawn_profile_harness_aria()}>
		{#each allAdapters as ad (ad)}
			<button
				type="button"
				class="harness-btn"
				class:on={draft.harness === ad}
				role="radio"
				aria-checked={draft.harness === ad}
				onclick={() => pickHarness(ad)}
			>
				<AdapterIcon adapter={ad} size={12} />
				{adapterLabel(ad)}
			</button>
		{/each}
	</div>

	<div class="pair">
		<Select
			size="sm"
			aria-label={m.spawn_profile_account_aria()}
			options={accountOptions}
			bind:value={() => accountPick, setAccountPick}
		/>
		<Select
			size="sm"
			aria-label={m.spawn_profile_model_aria()}
			options={modelOptions}
			bind:value={() => draft.model_alias ?? '', (v) => (draft.model_alias = v || null)}
		/>
	</div>

	<EffortSlider
		id="sp-profile-effort-{profile.id}"
		levels={efforts}
		current={draft.effort ?? ''}
		onset={(v) => (draft.effort = v || null)}
	/>

	<SegmentedControl
		size="sm"
		block
		label={m.spawn_permission_mode_label()}
		options={modeOptions}
		bind:value={() => draft.permission_mode ?? '', (v) => (draft.permission_mode = v || null)}
	/>

	<div class="foot">
		<Text size="xs" tone="faint">{m.spawn_profile_changes({ count: changes })}</Text>
		<span class="spacer"></span>
		<Button size="sm" disabled={busy} onclick={() => onuseonce({ ...draft })}>
			{m.spawn_profile_use_once()}
		</Button>
		<Button
			size="sm"
			variant="primary"
			disabled={busy || !name.trim()}
			onclick={() => onsave(name.trim(), { ...draft })}
		>
			{m.spawn_profile_save()}
		</Button>
	</div>
</div>

<style>
	.panel {
		border-top: 1px solid var(--border);
		background: var(--bg-elevated);
		padding: var(--sp-3);
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.name-row {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.harness {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: var(--sp-2);
	}
	.harness-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: var(--sp-2);
		height: var(--control-height-compact);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		background: transparent;
		color: var(--text-muted);
		font: inherit;
		font-size: var(--fs-sm);
		cursor: pointer;
	}
	.harness-btn.on {
		border-color: var(--accent);
		background: color-mix(in srgb, var(--accent) 10%, transparent);
		color: var(--text);
		font-weight: var(--fw-medium);
	}
	.harness-btn:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 1px;
	}
	.pair {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(0, 1.3fr);
		gap: var(--sp-2);
	}
	.foot {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.spacer {
		flex: 1;
	}
</style>
