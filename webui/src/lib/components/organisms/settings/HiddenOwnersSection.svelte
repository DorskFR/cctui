<script lang="ts">
	import type { HiddenOwner } from '@bindings/HiddenOwner';
	import { Button, EmptyState } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import { errMessage } from '$lib/api';
	import { ownerLabel, sortedOwners } from '$lib/hiddenOwners';
	import { useHiddenOwnerActions, useHiddenOwners } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';

	let { isAdmin = false }: { isAdmin?: boolean } = $props();

	const owners = useHiddenOwners(() => isAdmin);
	const actions = useHiddenOwnerActions();
	const list = $derived(sortedOwners(owners.data?.owners));
	let busy = $state<string | null>(null);

	const keyOf = (o: HiddenOwner) => `${o.kind}:${o.target_id}`;
	const kindLabel = (o: HiddenOwner) =>
		o.kind === 'user' ? m.hidden_owners_kind_user() : m.hidden_owners_kind_machine();

	async function unhide(o: HiddenOwner) {
		busy = keyOf(o);
		try {
			await actions.unhide(o.kind, o.target_id);
			toasts.ok(m.hidden_owners_unhidden_toast({ name: ownerLabel(o) }));
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			busy = null;
		}
	}
</script>

{#if isAdmin}
	<SettingSection
		id="hidden-owners"
		icon="◌"
		title={m.hidden_owners_settings_title()}
		description={m.hidden_owners_settings_desc()}
		admin
	>
		{#if owners.isPending}
			<EmptyState size="compact" loading title={m.common_loading()} />
		{:else if list.length === 0}
			<EmptyState size="compact" icon="eye" title={m.hidden_owners_settings_empty()} data-journey="hidden-owners-empty" />
		{:else}
			<SettingGroup>
				{#each list as o (keyOf(o))}
					<SettingRow label={ownerLabel(o)} help={kindLabel(o)} journey="hidden-owner" journeyKey={keyOf(o)} selfLabelled>
						<Button
							size="sm"
							loading={busy === keyOf(o)}
							aria-label={m.hidden_owners_unhide_named({ name: ownerLabel(o) })}
							onclick={() => unhide(o)}>{m.hidden_owners_unhide()}</Button
						>
					</SettingRow>
				{/each}
			</SettingGroup>
		{/if}
	</SettingSection>
{/if}
