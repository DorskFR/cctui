<script lang="ts">
	import { Button, Text } from '@dorsk/tsumikit';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import SettingSection from '#lib/components/molecules/SettingSection.svelte';
	import SharedByAccount from './SharedByAccount.svelte';
	import { useAccounts, useMe } from '#lib/queries/index.ts';
	import { accountHref } from '#lib/components/organisms/accounts/account-page/account-nav.logic.ts';
	import { ownedAccounts, sharedWithMe } from './accountSettings.logic';
	import { m } from '#lib/paraglide/messages.js';

	let { active = false }: { active?: boolean } = $props();

	const me = useMe();
	const accounts = useAccounts();
	const mine = $derived(ownedAccounts(accounts.data, me.data?.user_id));
	const theirs = $derived(sharedWithMe(accounts.data, me.data?.user_id));
</script>

<SettingSection id="sharing" icon="⇄" title={m.settings_sec_sharing()} description={m.settings_sharing_desc()}>
	<SettingGroup title={m.settings_sharing_by_me()}>
		{#if active}
			{#each mine as a (a.id)}
				<SharedByAccount account={a} />
			{/each}
		{/if}
		<SettingRow label={m.settings_sharing_manage()} help={m.settings_sharing_manage_help()} selfLabelled />
	</SettingGroup>
	<SettingGroup title={m.settings_sharing_with_me()}>
		{#each theirs as a (a.id)}
			<SettingRow
				label={a.emoji ? `${a.emoji} ${a.name}` : a.name}
				selfLabelled
				journey="shared-with-me"
				journeyKey={a.id}
			>
				<span class="from">
					<Text size="sm" tone="faint">{m.settings_sharing_from({ name: a.user_name ?? a.user_id })}</Text>
					<Button size="sm" variant="ghost" as="a" href={accountHref(a.id)}>{m.access_open()}</Button>
				</span>
			</SettingRow>
		{:else}
			<SettingRow label={m.settings_sharing_with_me_none()} selfLabelled wide />
		{/each}
	</SettingGroup>
</SettingSection>

<style>
	.from {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
	}
</style>
