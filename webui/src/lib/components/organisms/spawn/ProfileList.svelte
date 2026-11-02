<script lang="ts">
	import type { SessionProfile } from '@bindings/SessionProfile';
	import type { AccountPoolView } from '@bindings/AccountPoolView';
	import type { AccountUsageEntry, OAuthAccount } from '$lib/queries';
	import { Button, IconButton, RichSelect } from '@dorsk/tsumikit';
	import ProfileRow from './ProfileRow.svelte';
	import ProfileForm from './ProfileForm.svelte';
	import { harnessModelsFallback } from '$lib/domainTables';
	import { EMPTY_SPEC, sortProfiles, specChain, specOf, type ProfileSpecForm } from './profiles';
	import { badgeLook } from './accountBadge';
	import { m } from '$lib/paraglide/messages';

	let {
		profiles,
		selectedId = $bindable(),
		oneOff = $bindable(),
		accounts,
		pools = [],
		usage,
		machineId,
		busy = false,
		oncreate,
		onsave,
		ondelete
	}: {
		profiles: SessionProfile[];
		selectedId: string | null;
		oneOff: ProfileSpecForm | null;
		accounts: OAuthAccount[];
		pools?: AccountPoolView[];
		usage: AccountUsageEntry[];
		machineId: string;
		busy?: boolean;
		/** Resolves true once the profile exists and is selected. */
		oncreate: (name: string, spec: ProfileSpecForm) => Promise<boolean>;
		onsave: (id: string, name: string, spec: ProfileSpecForm) => void;
		ondelete: (id: string) => void;
	} = $props();

	type FormState = { mode: 'edit'; id: string } | { mode: 'create' } | null;

	let form = $state<FormState>(null);
	let draft = $state<ProfileSpecForm>({ ...EMPTY_SPEC });
	let listOpen = $state(false);

	const sorted = $derived(sortProfiles(profiles));
	const current = $derived(profiles.find((p) => p.id === selectedId) ?? null);
	const editing = $derived.by(() => {
		const f = form;
		return f?.mode === 'edit' ? (profiles.find((p) => p.id === f.id) ?? null) : null;
	});
	const creating = $derived(form?.mode === 'create');
	const attached = $derived(listOpen || !!editing || creating);

	let pickEl = $state<HTMLElement | null>(null);

	// RichSelect styles its panel `display: flex`, which outranks the UA rule
	// hiding a closed popover: a closed panel stays laid out, invisible but
	// catching taps. Hide it until RichSelect scopes that rule to :popover-open.
	$effect(() => {
		const panel = pickEl?.querySelector<HTMLElement>('.rs-panel');
		if (!panel) return;
		let shown = false;
		const apply = () => {
			const want = shown ? '' : 'none';
			if (panel.style.display !== want) panel.style.display = want;
		};
		const onToggle = (e: Event) => {
			shown = (e as ToggleEvent).newState === 'open';
			apply();
		};
		// Popover rewrites the panel's style attribute when the trigger resizes.
		const mo = new MutationObserver(apply);
		mo.observe(panel, { attributes: true, attributeFilter: ['style'] });
		panel.addEventListener('beforetoggle', onToggle);
		panel.addEventListener('toggle', onToggle);
		apply();
		return () => {
			mo.disconnect();
			panel.removeEventListener('beforetoggle', onToggle);
			panel.removeEventListener('toggle', onToggle);
		};
	});

	// The list and the form are exclusive: opening the list discards the form.
	$effect(() => {
		if (listOpen) form = null;
	});

	// With no profile the kit IS the one-off spec the spawn will use, so the
	// form binds straight to it. Seed it once: mirroring it into a second
	// $state and syncing with an effect re-enters until the tab dies.
	$effect(() => {
		if (profiles.length === 0 && oneOff === null) oneOff = { ...EMPTY_SPEC };
	});

	const chainLabels = $derived({
		auto: m.spawn_account_auto(),
		noAccount: m.spawn_account_none(),
		defaultModel: m.spawn_model_default(),
		defaultEffort: m.spawn_effort_default(),
		defaultMode: m.spawn_mode_default_label()
	});
	// The badge carries the emoji, so the chain names the account in text only.
	const plainAccounts = $derived(accounts.map((a) => ({ ...a, emoji: null })));
	const modelLabel = (harness: string, alias: string) =>
		harness === 'codex'
			? alias
			: (harnessModelsFallback('claude-code').models.find((x) => x.v === alias)?.label ?? alias);
	const chainOf = (spec: ProfileSpecForm) =>
		specChain(spec, plainAccounts, pools, chainLabels, modelLabel);
	const badgeOf = (spec: ProfileSpecForm) => badgeLook(spec, accounts, pools);
	const specFor = (p: SessionProfile) => (selectedId === p.id && oneOff ? oneOff : specOf(p));

	function select(id: string) {
		if (id !== selectedId) oneOff = null;
		selectedId = id;
		form = null;
	}
	function toggleEdit() {
		if (form?.mode === 'edit' || !current) {
			form = null;
			return;
		}
		draft = { ...specFor(current) };
		form = { mode: 'edit', id: current.id };
		listOpen = false;
	}
	function openCreate() {
		draft = { ...(oneOff ?? (current ? specOf(current) : EMPTY_SPEC)) };
		form = { mode: 'create' };
		listOpen = false;
	}
	function useOnce(spec: ProfileSpecForm) {
		if (editing) selectedId = editing.id;
		oneOff = spec;
		form = null;
	}
	function save(name: string, spec: ProfileSpecForm) {
		if (!editing) return;
		const id = editing.id;
		if (selectedId === id) oneOff = null;
		form = null;
		onsave(id, name, spec);
	}
	function remove() {
		if (form?.mode !== 'edit') return;
		const id = form.id;
		form = null;
		ondelete(id);
	}
	async function create(name: string, spec: ProfileSpecForm) {
		if (await oncreate(name, spec)) form = null;
	}
</script>

<div class="list" data-journey="profiles">
	{#if profiles.length === 0}
		{#if oneOff}
			<div class="form standalone" data-journey="profile-form" data-mode="create">
				<ProfileForm
					mode="create"
					bind:draft={oneOff}
					{profiles}
					{accounts}
					{pools}
					{usage}
					{machineId}
					{busy}
					onuseonce={(s) => (oneOff = s)}
					oncreate={create}
				/>
			</div>
		{/if}
	{:else}
		<div class="card" class:attached bind:this={pickEl}>
			{#if creating}
				<div class="draft" data-journey="profile-draft">
					<ProfileRow id="new" name={m.spawn_profile_new()} chain={chainOf(draft)} badge={badgeOf(draft)} variant="draft" />
				</div>
			{:else}
				<RichSelect
					options={sorted}
					value={selectedId ?? undefined}
					onchange={(_, p) => select(p.id)}
					bind:open={listOpen}
					getKey={(p) => p.id}
					getLabel={(p) => p.name}
					label={m.spawn_profiles_aria()}
					placeholder={m.spawn_profiles_aria()}
				>
					{#snippet option({ option: p, inTrigger })}
						<ProfileRow
							id={p.id}
							name={p.name}
							chain={chainOf(specFor(p))}
							badge={badgeOf(specFor(p))}
							variant={inTrigger ? 'card' : 'row'}
						/>
					{/snippet}
					{#snippet footer({ close })}
						<Button
							data-journey="new-profile"
							variant="link"
							size="sm"
							disabled={busy}
							onclick={() => {
								close();
								openCreate();
							}}>{m.spawn_profile_new()}</Button
						>
					{/snippet}
				</RichSelect>
				{#if current}
					<span class="tools">
						<IconButton
							icon="settings"
							label={m.spawn_profile_adjust()}
							inline
							size={14}
							pressed={!!editing}
							aria-expanded={!!editing}
							onclick={toggleEdit}
						/>
						<span class="divider" aria-hidden="true"></span>
					</span>
				{/if}
			{/if}
		</div>
		{#if editing}
			{@const p = editing}
			{#key p.id}
				<div class="form" data-journey="profile-form" data-mode="edit" data-profile-id={p.id}>
					<ProfileForm
						mode="edit"
						profile={p}
						bind:draft
						{profiles}
						{accounts}
						{pools}
						{usage}
						{machineId}
						{busy}
						onuseonce={useOnce}
						onsave={save}
						ondelete={remove}
						oncancel={() => (form = null)}
					/>
				</div>
			{/key}
		{:else if creating}
			<div class="form" data-journey="profile-form" data-mode="create">
				<ProfileForm
					mode="create"
					bind:draft
					{profiles}
					{accounts}
					{pools}
					{usage}
					{machineId}
					{busy}
					onuseonce={useOnce}
					oncreate={create}
					oncancel={() => (form = null)}
				/>
			</div>
		{/if}
	{/if}
</div>

<style>
	.list {
		display: flex;
		flex-direction: column;
		--card-fill: color-mix(in srgb, var(--accent) 14%, var(--bg));
		--card-pad: var(--sp-2);
		--tools-w: calc(24px + var(--sp-2) * 2 + 1px);
	}
	.card {
		position: relative;
		min-width: 0;
	}
	/* The trigger is the whole card, so RichSelect sizes and anchors its panel
	   to the card. The cog overlays the trigger, in the gap left before the
	   chevron. */
	.card :global(.pop-trigger.rs-trigger) {
		gap: var(--sp-2);
		padding: var(--card-pad);
		border-color: var(--accent);
		background: var(--card-fill);
		box-shadow: none;
		color: var(--accent);
	}
	@media (hover: hover) and (pointer: fine) {
		.card :global(.pop-trigger.rs-trigger:hover:not(:disabled)) {
			background: color-mix(in srgb, var(--accent) 20%, var(--bg));
		}
	}
	.card :global(.rs-chevron) {
		margin-inline-start: var(--tools-w);
		color: color-mix(in srgb, var(--accent) 70%, var(--text-muted));
	}
	.card.attached :global(.pop-trigger.rs-trigger),
	.card.attached .draft {
		border-end-start-radius: 0;
		border-end-end-radius: 0;
	}
	/* Popover places the panel 6px off the trigger; pull it flush. */
	.card :global(.pop-panel.rs-panel) {
		margin-top: -6px;
		border: var(--edge-width) solid var(--accent);
		border-top: none;
		border-radius: 0 0 var(--r-md) var(--r-md);
	}
	.card :global(.pop-panel.rs-panel[data-side='top']) {
		margin-top: 6px;
		border-top: var(--edge-width) solid var(--accent);
		border-bottom: none;
		border-radius: var(--r-md) var(--r-md) 0 0;
	}
	.card:has(:global(.rs-panel[data-side='top']:popover-open)) :global(.pop-trigger.rs-trigger) {
		border-radius: 0 0 var(--r-md) var(--r-md);
	}
	.card :global(.rs-row[data-selected]::before) {
		display: none;
	}
	.card :global(.rs-footer) {
		align-items: flex-start;
	}
	.tools {
		position: absolute;
		inset-block: 0;
		inset-inline-end: calc(var(--card-pad) + var(--control-icon-md) + var(--sp-2));
		width: var(--tools-w);
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: var(--sp-2);
		color: var(--accent);
		pointer-events: none;
	}
	.tools > :global(*) {
		pointer-events: auto;
	}
	.divider {
		align-self: stretch;
		width: 1px;
		margin-block: var(--sp-2);
		background: color-mix(in srgb, var(--accent) 35%, transparent);
	}
	.draft {
		padding: var(--card-pad);
		border: var(--edge-width) dashed var(--accent);
		border-radius: var(--r-md);
		background: var(--card-fill);
	}
	.form {
		border: var(--edge-width) solid var(--accent);
		border-top: none;
		border-radius: 0 0 var(--r-md) var(--r-md);
		overflow: hidden;
	}
	.form.standalone {
		border: var(--edge-width) solid var(--border);
		border-radius: var(--r-md);
	}
</style>
