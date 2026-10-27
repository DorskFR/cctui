<script lang="ts">
	import { Button, Callout, ConfirmModal, Field, IconButton, Input, Text, Timestamp } from '@dorsk/tsumikit';
	import { useUserActions, useUserDispatchPolicy } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { errMessage } from '$lib/api';
	import { m } from '$lib/paraglide/messages';
	import {
		blankPack,
		formFromRules,
		incompletePacks,
		invalidLimits,
		type LimitField,
		type PolicyForm,
		rulesFromForm
	} from './dispatchPolicy.logic';

	let {
		userId,
		userName,
		canManage
	}: {
		userId: string;
		userName: string;
		canManage: boolean;
	} = $props();

	const policy = useUserDispatchPolicy(
		() => userId,
		() => true
	);
	const actions = useUserActions();

	let form = $state<PolicyForm>(formFromRules(null));
	let loadedFor = $state<string | null>(null);
	let saving = $state(false);
	let confirmRemove = $state(false);

	// Reload the editor when the query lands or the user changes; edits in
	// progress are kept until the server answers.
	$effect(() => {
		const data = policy.data;
		const key = `${userId}:${data?.updated_at ?? 'none'}`;
		if (policy.isSuccess && loadedFor !== key) {
			form = formFromRules(data?.rules ?? null);
			loadedFor = key;
		}
	});

	const badLimits = $derived(invalidLimits(form));
	const invalid = $derived(incompletePacks(form) || badLimits.length > 0);
	const limitError = (f: LimitField) => (badLimits.includes(f) ? m.access_policy_limit_invalid() : undefined);

	async function save() {
		saving = true;
		try {
			await actions.setDispatchPolicy(userId, rulesFromForm(form));
			toasts.ok(m.access_policy_saved());
		} catch (e: unknown) {
			toasts.error(errMessage(e));
		} finally {
			saving = false;
		}
	}
	async function remove() {
		confirmRemove = false;
		try {
			await actions.clearDispatchPolicy(userId);
			toasts.ok(m.access_policy_removed());
		} catch (e: unknown) {
			toasts.error(errMessage(e));
		}
	}
	function addPack() {
		form.packs = [...form.packs, blankPack()];
	}
	function removePack(i: number) {
		form.packs = form.packs.filter((_, idx) => idx !== i);
	}
</script>

<div class="policy">
	<Callout tone="info">{m.access_policy_help()}</Callout>

	{#if policy.isSuccess}
		<div class="state">
			{#if policy.data}
				<Text size="sm" tone="muted">{m.access_policy_updated()}</Text>
				<Timestamp value={policy.data.updated_at} mode="relative" size="sm" tone="muted" />
			{:else}
				<Text size="sm" tone="muted">{m.access_policy_none()}</Text>
			{/if}
		</div>
	{/if}

	<div class="grid">
		<Field label={m.access_policy_dispatchers()} hint={m.access_policy_list_hint()}>
			<Input mono bind:value={form.dispatchers} disabled={!canManage} placeholder="cctui-kube" />
		</Field>
		<Field label={m.access_policy_profiles()} hint={m.access_policy_list_hint()}>
			<Input mono bind:value={form.profiles} disabled={!canManage} />
		</Field>
		<Field label={m.access_policy_adapters()} hint={m.access_policy_list_hint()}>
			<Input mono bind:value={form.adapters} disabled={!canManage} placeholder="claude-code, codex" />
		</Field>
		<Field label={m.access_policy_models()} hint={m.access_policy_list_hint()}>
			<Input mono bind:value={form.models} disabled={!canManage} />
		</Field>
	</div>

	<div class="packs">
		<Text as="p" size="sm" tone="muted">{m.access_policy_packs()}</Text>
		{#each form.packs as pack, i (i)}
			<div class="pack">
				<Input mono bind:value={pack.name} disabled={!canManage} placeholder={m.access_policy_pack_name()} />
				<Input mono grow bind:value={pack.url} disabled={!canManage} placeholder={m.access_policy_pack_url()} />
				<Input mono bind:value={pack.ref} disabled={!canManage} placeholder={m.access_policy_pack_ref()} />
				<Input mono bind:value={pack.subdir} disabled={!canManage} placeholder={m.access_policy_pack_subdir()} />
				{#if canManage}
					<IconButton
						inline
						hoverDanger
						icon="trash"
						size={14}
						label={m.access_policy_pack_remove()}
						title={m.access_policy_pack_remove()}
						onclick={() => removePack(i)}
					/>
				{/if}
			</div>
		{/each}
		{#if canManage}
			<div>
				<Button size="sm" onclick={addPack}>{m.access_policy_pack_add()}</Button>
			</div>
		{/if}
	</div>

	<div class="grid limits">
		<Field label={m.access_policy_max_timeout()} hint={m.access_policy_limit_hint()} error={limitError('maxTimeout')}>
			<Input mono inputmode="numeric" bind:value={form.maxTimeout} disabled={!canManage} />
		</Field>
		<Field label={m.access_policy_max_concurrent()} hint={m.access_policy_limit_hint()} error={limitError('maxConcurrent')}>
			<Input mono inputmode="numeric" bind:value={form.maxConcurrent} disabled={!canManage} />
		</Field>
		<Field label={m.access_policy_daily_cap()} hint={m.access_policy_limit_hint()} error={limitError('dailyCap')}>
			<Input mono inputmode="numeric" bind:value={form.dailyCap} disabled={!canManage} />
		</Field>
	</div>

	{#if canManage}
		<div class="actions">
			<Button variant="primary" disabled={saving || invalid || !policy.isSuccess} onclick={save}>
				{m.access_policy_save()}
			</Button>
			{#if policy.data}
				<Button variant="danger" onclick={() => (confirmRemove = true)}>{m.access_policy_remove()}</Button>
			{/if}
		</div>
	{/if}
</div>

{#if confirmRemove}
	<ConfirmModal
		open
		tone="danger"
		title={m.access_policy_remove()}
		message={m.access_policy_confirm_remove({ name: userName })}
		confirmLabel={m.access_policy_remove_confirm()}
		onconfirm={remove}
		oncancel={() => (confirmRemove = false)}
	/>
{/if}

<style>
	.policy {
		display: flex;
		flex-direction: column;
		gap: var(--sp-4);
		padding-top: var(--sp-3);
	}
	.state {
		display: flex;
		align-items: center;
		gap: var(--sp-1);
	}
	.grid {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--sp-3);
	}
	.limits {
		grid-template-columns: repeat(3, minmax(0, 1fr));
	}
	@media (max-width: 47.999rem) {
		.grid,
		.limits {
			grid-template-columns: 1fr;
		}
	}
	.packs {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.pack {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.actions {
		display: flex;
		gap: var(--sp-2);
	}
</style>
