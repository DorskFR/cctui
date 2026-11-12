<script lang="ts">
	// One editor for an existing profile (`edit`) and an unsaved one (`create`):
	// the shared kit plus what only a profile has, a name and pinned context.
	import type { SessionProfile } from '@bindings/SessionProfile';
	import type { AccountPoolView } from '@bindings/AccountPoolView';
	import type { AccountUsageEntry, OAuthAccount } from '$lib/queries';
	import { Button, Field, Input, Text } from '@dorsk/tsumikit';
	import KitFields from './KitFields.svelte';
	import ContextPicker from '$lib/components/molecules/ContextPicker.svelte';
	import { useContextItems } from '$lib/queries';
	import { hasFeature } from '$lib/features.svelte';
	import { profileNameTaken, specChanges, specOf, type ProfileSpecForm } from './profiles';
	import { m } from '$lib/paraglide/messages';

	let {
		mode,
		profile = null,
		draft = $bindable(),
		profiles = [],
		accounts,
		pools = [],
		usage,
		machineId,
		busy = false,
		onuseonce,
		onsave,
		oncreate,
		ondelete,
		oncancel
	}: {
		mode: 'edit' | 'create';
		/** The profile under edit; unused in create mode. */
		profile?: SessionProfile | null;
		draft: ProfileSpecForm;
		/** Every saved profile, for the unique-name check. */
		profiles?: readonly SessionProfile[];
		accounts: OAuthAccount[];
		pools?: AccountPoolView[];
		usage: AccountUsageEntry[];
		machineId: string;
		busy?: boolean;
		onuseonce: (spec: ProfileSpecForm) => void;
		onsave?: (name: string, spec: ProfileSpecForm) => void;
		oncreate?: (name: string, spec: ProfileSpecForm) => void;
		ondelete?: () => void;
		/** Discard: Esc, the Cancel button in create mode. */
		oncancel?: () => void;
	} = $props();

	const editing = $derived(mode === 'edit' && profile ? profile : null);
	const idSuffix = $derived(editing?.id ?? 'new');

	// svelte-ignore state_referenced_locally
	let name = $state(mode === 'edit' ? (profile?.name ?? '') : '');

	const contextOn = $derived(hasFeature('context'));
	const contextQ = useContextItems(() => contextOn);
	const pinnable = $derived(
		(contextQ.data ?? []).filter((i) => i.kind === 'memory' || i.kind === 'prompt')
	);

	const trimmed = $derived(name.trim());
	const nameOk = $derived(!!trimmed && !profileNameTaken(trimmed, profiles, editing?.id ?? null));
	const changes = $derived(
		editing ? specChanges(specOf(editing), draft) + (trimmed !== editing.name ? 1 : 0) : 0
	);

	function onkeydown(e: KeyboardEvent) {
		if (e.key !== 'Escape' || !oncancel || e.defaultPrevented) return;
		e.preventDefault();
		e.stopPropagation();
		oncancel();
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="panel" data-mode={mode} {onkeydown}>
	<Field label={m.spawn_profile_name_aria()} for="sp-profile-name-{idSuffix}">
		<Input
			id="sp-profile-name-{idSuffix}"
			bind:value={name}
			placeholder={m.spawn_profile_name_aria()}
		/>
	</Field>
	{#if trimmed && !nameOk}
		<Text size="xs" tone="danger">{m.spawn_profile_name_taken()}</Text>
	{/if}

	<KitFields bind:draft {accounts} {pools} {usage} {machineId} {idSuffix} />

	{#if contextOn}
		<Field label={m.spawn_profile_context_label()} for="sp-profile-context-{idSuffix}">
			<ContextPicker
				id="sp-profile-context-{idSuffix}"
				items={pinnable}
				by="id"
				bind:value={() => draft.context_items ?? [], (v) => (draft.context_items = v)}
				label={m.spawn_profile_context_label()}
			/>
		</Field>
	{/if}

	<div class="foot">
		{#if editing}
			<Button size="sm" variant="danger" disabled={busy} onclick={ondelete}>
				{m.spawn_profile_delete()}
			</Button>
			<Text size="xs" tone="faint">{m.spawn_profile_changes({ count: changes })}</Text>
		{:else if oncancel}
			<Button size="sm" variant="ghost" disabled={busy} onclick={oncancel}>
				{m.common_cancel()}
			</Button>
		{/if}
		<div class="foot-actions">
			<Button size="sm" disabled={busy} onclick={() => onuseonce({ ...draft })}>
				{m.spawn_profile_use_once()}
			</Button>
			{#if editing}
				<Button
					size="sm"
					variant="primary"
					disabled={busy || !nameOk}
					onclick={() => onsave?.(trimmed, { ...draft })}
				>
					{m.spawn_profile_save()}
				</Button>
			{:else}
				<Button
					size="sm"
					variant="primary"
					disabled={busy || !nameOk}
					onclick={() => oncreate?.(trimmed, { ...draft })}
				>
					{m.spawn_profile_create()}
				</Button>
			{/if}
		</div>
	</div>
</div>

<style>
	.panel {
		background: var(--bg-elevated);
		padding: var(--sp-3);
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	/* Three localized buttons don't always fit beside the left group in a
	   narrow panel: the action group wraps under it, right-aligned. */
	.foot {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
	}
	.foot-actions {
		display: flex;
		flex-wrap: wrap;
		justify-content: flex-end;
		gap: var(--sp-2);
		margin-left: auto;
	}
</style>
