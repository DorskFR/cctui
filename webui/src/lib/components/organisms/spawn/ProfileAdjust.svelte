<script lang="ts">
	// The profile editor: the shared kit organism plus the two things only a
	// profile has — a name and the save actions.
	import type { SessionProfile } from '@bindings/SessionProfile';
	import type { AccountPoolView } from '@bindings/AccountPoolView';
	import type { AccountUsageEntry, OAuthAccount } from '$lib/queries';
	import { Button, Field, IconButton, Input, Text } from '@dorsk/tsumikit';
	import KitFields from './KitFields.svelte';
	import { specChanges, specOf, type ProfileSpec } from './profiles';
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
		initial: ProfileSpec;
		accounts: OAuthAccount[];
		pools?: AccountPoolView[];
		usage: AccountUsageEntry[];
		machineId: string;
		busy?: boolean;
		onuseonce: (spec: ProfileSpec) => void;
		onsave: (name: string, spec: ProfileSpec) => void;
		ondelete?: () => void;
	} = $props();

	// svelte-ignore state_referenced_locally
	let draft = $state<ProfileSpec>({ ...initial });
	// svelte-ignore state_referenced_locally
	let name = $state(profile?.name ?? '');

	const saved = $derived(profile ? specOf(profile) : { ...initial });
	const changes = $derived(
		specChanges(saved, draft) + (profile && name.trim() !== profile.name ? 1 : 0)
	);
</script>

<div class="panel">
	<Field label={m.spawn_profile_name_aria()} for="sp-profile-name-{profile?.id ?? 'new'}">
		<div class="name-row">
			<Input
				id="sp-profile-name-{profile?.id ?? 'new'}"
				bind:value={name}
				placeholder={m.spawn_profile_name_aria()}
			/>
			{#if profile}
				<IconButton
					icon="trash"
					label={m.spawn_profile_delete()}
					inline
					hoverDanger
					onclick={ondelete}
				/>
			{/if}
		</div>
	</Field>

	<KitFields
		bind:draft
		{accounts}
		{pools}
		{usage}
		{machineId}
		idSuffix={profile?.id ?? 'new'}
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
	.foot {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.spacer {
		flex: 1;
	}
</style>
