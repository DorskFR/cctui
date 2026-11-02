<script lang="ts">
	import type { SessionProfile } from '@bindings/SessionProfile';
	import type { AccountPoolView } from '@bindings/AccountPoolView';
	import type { AccountUsageEntry, OAuthAccount } from '$lib/queries';
	import { Button, IconButton, RichSelect } from '@dorsk/tsumikit';
	import ProfileRow from './ProfileRow.svelte';
	import ProfileAdjust from './ProfileAdjust.svelte';
	import KitFields from './KitFields.svelte';
	import { harnessModelsFallback } from '$lib/domainTables';
	import { EMPTY_SPEC, moveProfile, specChain, specOf, type ProfileSpecForm } from './profiles';
	import { useProfileActions } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { errMessage } from '$lib/api';
	import { profileUsage } from '$lib/spawnMemory';
	import { m } from '$lib/paraglide/messages';

	let {
		profiles,
		selectedId = $bindable(),
		oneOff = $bindable(),
		accounts,
		pools = [],
		usage,
		usageRaw,
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
		usageRaw: string;
		machineId: string;
		busy?: boolean;
		oncreate: () => void;
		onsave: (id: string, name: string, spec: ProfileSpecForm) => void;
		ondelete: (id: string) => void;
	} = $props();

	let openId = $state<string | null>(null);
	let pending = $state<SessionProfile[] | null>(null);
	const actions = useProfileActions();

	const ordered = $derived(pending ?? profiles);
	const adjusting = $derived(ordered.find((p) => p.id === openId) ?? null);

	async function persist(next: SessionProfile[]) {
		const ids = next.map((p) => p.id);
		if (ids.join() === profiles.map((p) => p.id).join()) return;
		pending = next;
		try {
			await actions.reorder(ids);
		} catch (e) {
			toasts.error(m.spawn_profile_reorder_failed({ error: errMessage(e) }));
		} finally {
			pending = null;
		}
	}

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

	function reorder(from: number, to: number) {
		const p = ordered[from];
		if (p) void persist(moveProfile(ordered, p.id, to));
	}

	// With no profile the kit IS the one-off spec the spawn will use, so the
	// editor binds straight to it. Seed it once — mirroring it into a second
	// $state and syncing with an effect writes the parent on every run and
	// re-enters until the tab dies.
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
	const modelLabel = (harness: string, alias: string) =>
		harness === 'codex'
			? alias
			: (harnessModelsFallback('claude-code').models.find((x) => x.v === alias)?.label ?? alias);

	function usageText(id: string): string {
		const u = profileUsage(usageRaw, id);
		if (!u) return '';
		if ('week' in u) return m.spawn_profile_uses_week({ count: u.week });
		const days = Math.floor((Date.now() - u.lastAt) / 86_400_000);
		return days < 1 ? m.spawn_profile_last_used_today() : m.spawn_profile_last_used_days({ days });
	}

	function select(id: string) {
		if (id !== selectedId) oneOff = null;
		selectedId = id;
	}
	function toggle(id: string) {
		openId = openId === id ? null : id;
	}
	const specFor = (p: SessionProfile) => (selectedId === p.id && oneOff ? oneOff : specOf(p));
	function useOnce(p: SessionProfile, spec: ProfileSpecForm) {
		selectedId = p.id;
		oneOff = spec;
		openId = null;
	}
	function save(p: SessionProfile, name: string, spec: ProfileSpecForm) {
		if (selectedId === p.id) oneOff = null;
		openId = null;
		onsave(p.id, name, spec);
	}
</script>

<div class="list" data-journey="profiles">
	<!-- No saved profile yet: the bare kit stands in for the profile picker, so
	     harness / account / model / effort / permissions are always reachable
	     and a session can be started without creating a profile first. -->
	{#if profiles.length === 0}
		{#if oneOff}
			<KitFields bind:draft={oneOff} {accounts} {pools} {usage} {machineId} />
		{/if}
		<div class="new">
			<Button data-journey="new-profile" variant="link" size="sm" disabled={busy} onclick={oncreate}>{m.spawn_profile_new()}</Button>
		</div>
	{:else}
		<div class="pick" bind:this={pickEl}>
			<RichSelect
				options={ordered}
				value={selectedId ?? undefined}
				onchange={(_, p) => select(p.id)}
				getKey={(p) => p.id}
				getLabel={(p) => p.name}
				onreorder={ordered.length > 1 ? reorder : undefined}
				movedMessage={(name, position, total) => m.spawn_profile_moved({ name, position, total })}
				label={m.spawn_profiles_aria()}
				placeholder={m.spawn_profiles_aria()}
				style="flex: 1 1 auto"
			>
				{#snippet option({ option: p, inTrigger })}
					<ProfileRow
						id={p.id}
						name={p.name}
						chain={specChain(specFor(p), accounts, pools, chainLabels, modelLabel)}
						usage={inTrigger ? '' : usageText(p.id)}
					/>
				{/snippet}
				{#snippet trailing({ option: p, close })}
					<IconButton
						icon="settings"
						label={m.spawn_profile_adjust()}
						inline
						size={14}
						pressed={openId === p.id}
						aria-expanded={openId === p.id}
						onclick={() => {
							close();
							toggle(p.id);
						}}
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
							oncreate();
						}}>{m.spawn_profile_new()}</Button
					>
				{/snippet}
			</RichSelect>
			{#if selectedId}
				{@const id = selectedId}
				<IconButton
					icon="settings"
					label={m.spawn_profile_adjust()}
					inline
					size={14}
					pressed={openId === id}
					aria-expanded={openId === id}
					onclick={() => toggle(id)}
				/>
			{/if}
		</div>
		{#if adjusting}
			{@const p = adjusting}
			{#key p.id}
				<div class="adjust" data-journey="profile-adjust" data-profile-id={p.id}>
					<ProfileAdjust
						profile={p}
						initial={specFor(p)}
						{accounts}
						{pools}
						{usage}
						{machineId}
						{busy}
						onuseonce={(s) => useOnce(p, s)}
						onsave={(name, s) => save(p, name, s)}
						ondelete={() => {
							openId = null;
							ondelete(p.id);
						}}
					/>
				</div>
			{/key}
		{/if}
	{/if}
</div>

<style>
	.list {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.pick {
		display: flex;
		align-items: center;
		gap: var(--sp-1);
		min-width: 0;
	}
	.adjust {
		border: 1px solid var(--border);
		border-radius: var(--r-md);
		background: var(--bg);
	}
	.new {
		display: flex;
		justify-content: flex-end;
		padding: 0 var(--sp-1);
	}
</style>
