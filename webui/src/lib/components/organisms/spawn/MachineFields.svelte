<script lang="ts">
	// The "Machine" branch of the spawn form: a machine · working-dir ·
	// label row, the prompt, then a collapsed configuration line (account ·
	// harness · model · effort · permission mode) that a gear expands into the
	// full editors for this session only.
	//
	// Account is the primary axis, but no longer a hard lock: the
	// harness cards are enabled per the selected account's provider-family union
	// — an account with anthropic+openai providers offers both, a
	// single-provider account disables the missing card. The provider credential
	// backing the effective harness drives the model list + aliases.
	import type { MachineRow } from '@bindings/MachineRow';
	import type { OAuthAccount } from '$lib/queries';
	import type { AccountPoolView } from '@bindings/AccountPoolView';
	import { useCodexModels, useMergedCodexModels, useGitInfo } from '$lib/queries';
	import ModelPicker from '$lib/components/molecules/ModelPicker.svelte';
	import CodexModelsRefresh from '$lib/components/molecules/CodexModelsRefresh.svelte';
	import { preferCatalog } from '$lib/harnessModels';
	import type { GitInfo } from '@bindings/GitInfo';
	import BrandLogo from '$lib/components/atoms/BrandLogo.svelte';
	import MachinePicker from '$lib/components/molecules/MachinePicker.svelte';
	import {
		Badge,
		Field,
		FilterInput,
		Icon,
		IconButton,
		Input,
		OptionButton,
		Select,
		Text,
		Textarea,
		type Query
	} from '@dorsk/tsumikit';
	import { makeCwdSchema, cwdToQuery, dirFromQuery } from './cwdSchema';
	import { gitBadge, makeGitInfoWatcher } from './cwdGitInfo';
	import EffortSlider from './EffortSlider.svelte';
	import {
		claudeModels,
		claudeEfforts,
		codexModelsFor,
		codexEffortsFor,
		modes,
		allAdapters,
		accountAdapters,
		providerForAdapter,
		isCompatibleProvider,
		withAliasTargets,
		adapterLabel,
		NO_ACCOUNT,
		poolName,
		poolValue,
		type Adapter
	} from './options';
	import { submitChordLabel, isSubmitChord } from '$lib/platform';
	import { makeClipboardFiles } from '$lib/attachments';
	import type { Form } from './types';
	import { m } from '$lib/paraglide/messages';

	let {
		form = $bindable(),
		machines,
		recentDirs,
		accounts,
		pools = [],
		onsubmit,
		onfiles,
		docked = false
	}: {
		form: Form;
		machines: MachineRow[];
		recentDirs: string[];
		// Every account the caller owns. The picker offers them all and
		// derives the allowed harnesses + model list from the chosen one.
		accounts: OAuthAccount[];
		// The caller's account pools. Offered above the individual accounts:
		// picking one delegates the choice the way Auto does, but bounded to
		// the pool's members.
		pools?: AccountPoolView[];
		// Submit the whole spawn form from the prompt textarea (Ctrl/⌘+Enter).
		onsubmit?: () => void;
		// Files pasted into the prompt textarea (Ctrl/⌘+V of a screenshot or a
		// copied file) go here instead of the "Add files" picker. Text pastes
		// are left to the browser.
		onfiles?: (files: File[]) => void;
		// Docked panel (always-visible form): the account/harness/model/effort/
		// permission editors stay expanded with no summary line to unfold, and
		// the permission-mode cards shrink to their name (no hint line).
		docked?: boolean;
	} = $props();

	// The machine picker + working-dir share one FilterInput: machine is
	// picked via the inline badge-tinted listbox; the dir is a single `cwd:` field whose async
	// provider serves recent dirs + live typeahead. `form.working_dir` is the
	// source of truth — the raw query mirrors it both ways, `lastDir` tracking
	// what the current query represents so the two syncs never loop.
	const cwdSchema = makeCwdSchema(
		() => form.machine_id,
		() => recentDirs,
		m.spawn_cwd_label()
	);
	const clipboardFiles = makeClipboardFiles();
	function onPromptPaste(e: ClipboardEvent) {
		if (!onfiles || !e.clipboardData) return;
		const files = clipboardFiles(e.clipboardData);
		if (files.length === 0) return;
		e.preventDefault();
		onfiles(files);
	}

	// svelte-ignore state_referenced_locally
	let cwdRaw = $state(cwdToQuery(form.working_dir));
	// svelte-ignore state_referenced_locally
	let lastDir = form.working_dir;
	function onCwdChange(q: Query) {
		const dir = dirFromQuery(q);
		// The input re-emits its unchanged query on mount and on rerenders; only
		// a real move away from what the field already held is a user edit, so
		// those echoes can't stomp a dir the modal prefilled meanwhile.
		if (dir === lastDir) return;
		lastDir = dir;
		form.working_dir = dir;
	}
	$effect(() => {
		const dir = form.working_dir;
		if (dir !== lastDir) {
			lastDir = dir;
			cwdRaw = cwdToQuery(dir);
		}
	});

	const fetchGitInfo = useGitInfo();
	let cwdGit = $state<GitInfo | null>(null);
	const cwdBadge = $derived(gitBadge(cwdGit));
	const gitWatcher = makeGitInfoWatcher(fetchGitInfo, (info) => (cwdGit = info));
	$effect(() => {
		gitWatcher.update(form.machine_id, form.working_dir);
		return gitWatcher.cancel;
	});
	const cwdBadgeTitle = $derived.by(() => {
		if (!cwdBadge) return '';
		if (cwdBadge.sha) return m.spawn_cwd_detached_title({ sha: cwdBadge.sha });
		if (cwdBadge.worktree) return m.spawn_cwd_worktree_title({ branch: cwdBadge.text });
		return m.spawn_cwd_branch_title({ branch: cwdBadge.text });
	});

	// Accounts are identities: matched by name. '' = Auto, the
	// NO_ACCOUNT sentinel = explicit unbound — both resolve to no selected
	// account here (Auto lets the server bind, unbound skips binding).
	// The pool the picker currently names, if any. A pool is neither a named
	// account nor "no account": the server elects a member, so nothing here can
	// name the credential in advance.
	const selectedPool = $derived(poolName(form.account));
	const selectedAccount = $derived(
		form.account && form.account !== NO_ACCOUNT && !selectedPool
			? accounts.find((a) => a.name === form.account)
			: undefined
	);

	// Harnesses the selected account can run (provider-family union); no
	// account = both. The harness in effect is always the user's pick
	// — never silently swapped; clicking a card the account can't back
	// drops to Auto instead.
	const allowedAdapters = $derived(selectedAccount ? accountAdapters(selectedAccount) : allAdapters);
	const effectiveAdapter = $derived(form.adapter_id);
	// The provider credential backing the picked harness on this account.
	const selectedProvider = $derived(providerForAdapter(selectedAccount, effectiveAdapter));

	// Auto (empty account) binds the single account whose provider family can run
	// the picked harness: resolve it client-side so the picker
	// can name the credential Auto would use, mirroring the server.
	const autoAccount = $derived.by(() => {
		const family = accounts.filter((a) => accountAdapters(a).includes(effectiveAdapter as Adapter));
		return family.length === 1 ? family[0].name : undefined;
	});

	// Model options for the chosen axis:
	//  * compatible account → its own declared models;
	//  * native account / Default → the harness's native families.
	const accountModelOptions = $derived(
		(selectedProvider?.models ?? []).map((m) => ({ v: m.model, label: m.label }))
	);
	const usesAccountModels = $derived(
		!!selectedProvider && isCompatibleProvider(selectedProvider.provider)
	);
	// Native claude families annotated with the selected account's alias targets.
	const claudeModelOptions = $derived(
		withAliasTargets(claudeModels, selectedProvider?.model_aliases)
	);

	// Codex catalog: the machine's own `model/list` report, else the
	// cross-machine merge, else the static offline list.
	const machineCodexCatalog = useCodexModels(() => (effectiveAdapter === 'codex' ? form.machine_id : ''));
	const mergedCodexCatalog = useMergedCodexModels(() => effectiveAdapter === 'codex');
	const codexCatalog = $derived(preferCatalog(machineCodexCatalog.data, mergedCodexCatalog.data));
	const codexModelOptions = $derived(codexModelsFor(codexCatalog));
	const codexEffortOptions = $derived(codexEffortsFor(codexCatalog, form.model_codex));

	// Clear a stale account selection if it no longer exists (e.g. accounts
	// reloaded); keep account_provider tracking the credential actually in use so
	// the spawn request stays unambiguous.
	$effect(() => {
		if (
			form.account &&
			form.account !== NO_ACCOUNT &&
			!accounts.some((a) => a.name === form.account)
		) {
			form.account = '';
		}
	});
	$effect(() => {
		form.account_provider = selectedProvider?.provider ?? '';
	});

	// Collapsed configuration line: one summary of the five knobs;
	// expansion is per-open only (not persisted). The knobs prefill from the
	// (machine, cwd) spawn memory (SpawnModal's memory effects).
	let configOpen = $state(false);
	const summaryModel = $derived(
		usesAccountModels
			? form.model_account
			: effectiveAdapter === 'codex'
				? form.model_codex
				: form.model_claude
	);
	const summaryEffort = $derived(
		effectiveAdapter === 'codex' ? form.effort_codex : form.effort_claude
	);
	const accountSummary = $derived(
		form.account === NO_ACCOUNT ? 'no account' : form.account || 'auto'
	);
	const configSummary = $derived(
		[
			accountSummary,
			effectiveAdapter,
			summaryModel || 'default',
			summaryEffort || 'default',
			form.permission_mode || 'default'
		].join(' · ')
	);

	// Docked panel: name-only permission cards, tighter than the default
	// label-over-hint card. Inline so no :global reach into the atom.
	const compactModeStyle = $derived(
		docked ? 'padding:var(--sp-1) var(--sp-2);font-size:var(--fs-sm);align-items:center;' : ''
	);

	// Per-mode accent: ask = green (safe), auto = blue (sandboxed),
	// yolo = red (no prompts, full access).
	const modeAccent: Record<string, string> = {
		ask: 'var(--c-green)',
		auto: 'var(--c-blue)',
		yolo: 'var(--c-red)',
		whip: 'var(--c-red)'
	};
</script>

<!-- Machine badge + working dir live in one structured field; the
     name sits below. The label auto-fills from the (machine, cwd) memory
     (in SpawnModal). -->
<div class="top-stack">
	<Field label={m.spawn_cwd_label()}>
		<FilterInput
			schema={cwdSchema}
			bind:value={cwdRaw}
			icon={null}
			showClear={false}
			placeholder="/home/user/project"
			onchange={onCwdChange}
		>
			{#snippet inline()}
				<MachinePicker bind:value={form.machine_id} {machines} label={m.spawn_machine_label()} />
			{/snippet}
		</FilterInput>
		{#if cwdBadge}
			<div style="margin-top:var(--sp-1)">
				<Badge
					mono
					title={cwdBadgeTitle}
					style="display:inline-flex;align-items:center;gap:0.25em;min-width:0;max-width:100%"
				>
					<Icon name="fork" size={12} label={m.sessions_branch_label()} />
					<span style="overflow:hidden;white-space:nowrap;text-overflow:ellipsis">{cwdBadge.text}{cwdBadge.worktree ? ` · ${m.spawn_cwd_worktree_badge()}` : ''}</span>
				</Badge>
			</div>
		{/if}
	</Field>

	<Field label={m.spawn_label_label()} for="sp-name">
		<Input id="sp-name" placeholder={m.spawn_session_label_placeholder()} bind:value={form.name} />
	</Field>
</div>

<Field label={m.spawn_prompt_label()} for="sp-prompt">
	<Textarea
		id="sp-prompt"
		style="min-height:8rem;max-height:14rem;resize:vertical;overflow-y:auto"
		placeholder={m.spawn_prompt_placeholder()}
		bind:value={form.prompt}
		autoresize
		onpaste={onPromptPaste}
		onkeydown={(e: KeyboardEvent) => {
			if (onsubmit && isSubmitChord(e)) {
				e.preventDefault();
				onsubmit();
			}
		}}
	/>
	<Text size="xs" tone="faint" style="display:block;margin-top:var(--sp-1)">{m.spawn_prompt_submit_hint({ chord: submitChordLabel() })}</Text>
</Field>

<!-- Account · harness · model · effort · permission mode, collapsed into one
     line; the gear expands the full editors. Field ORDER inside the
     expansion is stable regardless of selection. -->
<div class="config">
	{#if !docked}
	<div class="config-line">
		<Text size="sm" tone="faint" truncate title={configSummary}>{configSummary}</Text>
		<IconButton
			icon="settings"
			label={m.spawn_config_label()}
			aria-expanded={configOpen}
			pressed={configOpen}
			onclick={() => (configOpen = !configOpen)}
		/>
	</div>
	{/if}

	{#if configOpen || docked}
		<div class="config-fields">
			<Field label={m.spawn_account_label()} for="sp-account">
				<Select id="sp-account" bind:value={form.account}>
					<option value="">{m.spawn_account_auto()}{autoAccount ? ` — ${autoAccount}` : ''}</option>
					<option value={NO_ACCOUNT}>{m.spawn_account_none()}</option>
					{#if pools.length > 0}
						<!-- Pools first among the bounded choices: picking one is the
						     narrow instruction ("these accounts"), where Auto above is
						     the wide one ("any account I can reach"). -->
						<optgroup label={m.spawn_account_pool_group()}>
							{#each pools as p (p.id)}
								<option value={poolValue(p.name)}>{p.name}</option>
							{/each}
						</optgroup>
					{/if}
					{#each accounts as a (a.id)}
						<option value={a.name}>
							{a.name} ({a.providers.map((p) => p.provider).join(', ') || m.spawn_no_provider()})
						</option>
					{/each}
				</Select>
				{#if form.account === NO_ACCOUNT}
					<Text tone="faint" size="xs">{m.spawn_account_none_hint()}</Text>
				{:else if selectedPool}
					<Text tone="faint" size="xs">
						{m.spawn_account_pool_hint({ name: selectedPool })}
					</Text>
				{:else if selectedAccount}
					<Text tone="faint" size="xs">{m.spawn_account_gateway_hint()}</Text>
				{:else if autoAccount}
					<Text tone="faint" size="xs">{m.spawn_account_auto_hint({ account: autoAccount })}</Text>
				{:else}
					<Text tone="faint" size="xs">{m.spawn_account_auto_fallback_hint()}</Text>
				{/if}
			</Field>

			<!-- Harness: same two cards always; a card is disabled when the selected
			     account has no provider of that family, so the layout
			     never shifts. -->
			<Field label={m.spawn_field_harness()}>
				<div class="adapters">
					{#each allAdapters as ad (ad)}
						<OptionButton
							row
							selected={form.adapter_id === ad}
							style="--opt-accent: {ad === 'codex' ? 'var(--c-blue)' : 'var(--c-amber)'}"
							onclick={() => {
								form.adapter_id = ad;
								if (selectedAccount && !allowedAdapters.includes(ad)) form.account = '';
							}}
						>
							<BrandLogo adapter={ad} size={18} />
							<Text>{adapterLabel(ad)}</Text>
						</OptionButton>
					{/each}
				</div>
				{#if selectedAccount && !allowedAdapters.includes(form.adapter_id as Adapter)}
					<Text size="xs" style="color:var(--c-red)">
						{m.spawn_harness_no_credential({
							account: form.account,
							harness: adapterLabel(form.adapter_id)
						})}
					</Text>
				{/if}
			</Field>

			<!-- Model: driven by the effective harness — the account's own models for
			     a compatible endpoint, else the harness's native families. -->
			<Field label={m.spawn_field_model()} for="sp-model">
				{#if usesAccountModels}
					<ModelPicker
						id="sp-model"
						bind:value={form.model_account}
						options={accountModelOptions.length ? accountModelOptions : [{ v: '', label: m.spawn_model_default() }]}
					/>
				{:else if effectiveAdapter === 'codex'}
					<div class="model-row">
						<ModelPicker id="sp-model" bind:value={form.model_codex} options={codexModelOptions} />
						{#if form.machine_id}<CodexModelsRefresh machineId={form.machine_id} />{/if}
					</div>
				{:else}
					<ModelPicker id="sp-model" bind:value={form.model_claude} options={claudeModelOptions} />
				{/if}
			</Field>

			<!-- Per-adapter effort: keyed off the effective harness so an
			     account-limited codex still shows codex efforts. -->
			{#if effectiveAdapter === 'codex'}
				<EffortSlider
					id="sp-effort-codex"
					levels={codexEffortOptions}
					current={form.effort_codex}
					onset={(v) => (form.effort_codex = v)}
				/>
			{:else}
				<EffortSlider
					id="sp-effort-claude"
					levels={claudeEfforts}
					current={form.effort_claude}
					onset={(v) => (form.effort_claude = v)}
				/>
			{/if}

			<Field label={m.spawn_permission_mode_label()}>
				<div class="modes" class:compact={docked}>
					<!-- "Default" (unset) leaves the mode to claude's own default — no mode
					     is forced into the spawn. -->
					<OptionButton
						selected={form.permission_mode === ''}
						style={compactModeStyle || undefined}
						onclick={() => (form.permission_mode = '')}
					>
						<strong>{m.spawn_mode_default_label()}</strong>
						{#if !docked}<Text tone="faint" size="xs">{m.spawn_mode_default_hint()}</Text>{/if}
					</OptionButton>
					{#each modes as md (md.v)}
						<OptionButton
							selected={form.permission_mode === md.v}
							style={`--opt-accent: ${modeAccent[md.v]};${compactModeStyle}`}
							onclick={() => (form.permission_mode = md.v)}
						>
							<strong>{md.label}</strong>
							{#if !docked}<Text tone="faint" size="xs">{md.hint}</Text>{/if}
						</OptionButton>
					{/each}
				</div>
			</Field>
		</div>
	{/if}
</div>

<style>
	.model-row {
		display: flex;
		align-items: flex-start;
		gap: var(--sp-1);
	}
	.model-row > :global(:first-child) {
		flex: 1;
		min-width: 0;
	}

	.top-stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.config {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.config-line {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
		min-width: 0;
		padding: var(--sp-1) var(--sp-2);
		border: 1px solid var(--border);
		border-radius: var(--r-md);
	}
	.config-fields {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.acct-hint {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.adapters {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: var(--sp-2);
	}
	.modes {
		display: grid;
		grid-template-columns: 1fr 1fr 1fr;
		gap: var(--sp-2);
	}
	/* Compact (docked panel): name-only cards packed tighter. */
	.modes.compact {
		gap: var(--sp-1);
	}
</style>
