<script lang="ts">
	import type { SessionProfile } from '@bindings/SessionProfile';
	import type { AccountPoolView } from '@bindings/AccountPoolView';
	import type { AccountUsageEntry, OAuthAccount } from '$lib/queries';
	import ProfileRow from './ProfileRow.svelte';
	import ProfileForm from './ProfileForm.svelte';
	import ProfilePicker from './ProfilePicker.svelte';
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
		ondelete,
		manageHref
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
		/** Pick-only: no inline editing, the picker links here to manage profiles. */
		manageHref?: string;
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
		{#if creating}
			<div class="draft" data-journey="profile-draft">
				<ProfileRow id="new" name={m.spawn_profile_new()} chain={chainOf(draft)} badge={badgeOf(draft)} variant="draft" />
			</div>
		{:else}
			<ProfilePicker
				profiles={sorted}
				{selectedId}
				bind:open={listOpen}
				attached={!!editing}
				editing={!!editing}
				{busy}
				{specFor}
				{chainOf}
				{badgeOf}
				{manageHref}
				onselect={select}
				onedit={manageHref ? undefined : toggleEdit}
				onnew={manageHref ? undefined : openCreate}
			/>
		{/if}
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
	}
	.draft {
		padding: var(--sp-2);
		border: var(--edge-width) dashed var(--accent);
		border-end-start-radius: 0;
		border-end-end-radius: 0;
		border-start-start-radius: var(--r-md);
		border-start-end-radius: var(--r-md);
		background: color-mix(in srgb, var(--accent) 14%, var(--bg));
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
