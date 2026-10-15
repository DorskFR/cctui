<script lang="ts">
	// The profile editor: the shared kit organism plus what only a profile
	// has — a name, pinned context and the save actions.
	import type { SessionProfile } from '@bindings/SessionProfile';
	import type { AccountPoolView } from '@bindings/AccountPoolView';
	import type { AccountUsageEntry, OAuthAccount } from '$lib/queries';
	import { Button, Field, Input, Text } from '@dorsk/tsumikit';
	import KitFields from './KitFields.svelte';
	import ContextPicker from '$lib/components/molecules/ContextPicker.svelte';
	import { useContextItems } from '$lib/queries';
	import { specChanges, specOf, type ProfileSpecForm } from './profiles';
	import { m } from '$lib/paraglide/messages';

	let {
		profile = null,
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
		/** null = no saved profile yet: the panel edits an unsaved kit. */
		profile?: SessionProfile | null;
		initial: ProfileSpecForm;
		accounts: OAuthAccount[];
		pools?: AccountPoolView[];
		usage: AccountUsageEntry[];
		machineId: string;
		busy?: boolean;
		onuseonce: (spec: ProfileSpecForm) => void;
		onsave: (name: string, spec: ProfileSpecForm) => void;
		ondelete?: () => void;
	} = $props();

	// svelte-ignore state_referenced_locally
	let draft = $state<ProfileSpecForm>({ ...initial });
	// svelte-ignore state_referenced_locally
	let name = $state(profile?.name ?? '');

	const contextQ = useContextItems();
	const pinnable = $derived(
		(contextQ.data ?? []).filter((i) => i.kind === 'memory' || i.kind === 'prompt')
	);

	const saved = $derived(profile ? specOf(profile) : { ...initial });
	const changes = $derived(
		specChanges(saved, draft) + (profile && name.trim() !== profile.name ? 1 : 0)
	);
</script>

<div class="panel">
	<Field label={m.spawn_profile_name_aria()} for="sp-profile-name-{profile?.id ?? 'new'}">
		<Input
			id="sp-profile-name-{profile?.id ?? 'new'}"
			bind:value={name}
			placeholder={m.spawn_profile_name_aria()}
		/>
	</Field>

	<KitFields
		bind:draft
		{accounts}
		{pools}
		{usage}
		{machineId}
		idSuffix={profile?.id ?? 'new'}
	/>

	<Field label={m.spawn_profile_context_label()} for="sp-profile-context-{profile?.id ?? 'new'}">
		<ContextPicker
			id="sp-profile-context-{profile?.id ?? 'new'}"
			items={pinnable}
			by="id"
			bind:value={() => draft.context_items ?? [], (v) => (draft.context_items = v)}
			label={m.spawn_profile_context_label()}
		/>
	</Field>

	<div class="foot">
		<Text size="xs" tone="faint">{m.spawn_profile_changes({ count: changes })}</Text>
		<span class="spacer"></span>
		<div class="foot-actions">
		{#if profile}
			<Button size="sm" variant="danger" disabled={busy} onclick={ondelete}>
				{m.spawn_profile_delete()}
			</Button>
		{/if}
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
	/* Three localized buttons don't always fit beside the counter in a narrow
	   panel: the action group wraps under it (right-aligned) instead of the last
	   button being clipped. */
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
	.spacer {
		flex: 1;
	}
</style>
