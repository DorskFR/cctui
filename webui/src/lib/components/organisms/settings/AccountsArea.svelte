<script lang="ts">
	// Settings › Accounts: who you are, how you sign in, your machines and the
	// provider accounts sessions run on. The provider board (also `/accounts`)
	// and, for non-admins, their own keys and enrol card are screens rather
	// than rows, so they mount only while this area is shown.
	import { Button, Link, Text } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import AccessPanel from '$lib/components/organisms/access/AccessPanel.svelte';
	import AccountsPanel from '$lib/components/organisms/accounts/AccountsPanel.svelte';
	import MachineAdaptersGroup from './MachineAdaptersGroup.svelte';
	import SecuritySection from './SecuritySection.svelte';
	import { settingsApi } from './settingsApi';
	import { auth } from '$lib/auth.svelte';
	import { confirmDialog } from '$lib/confirmDialog.svelte';
	import { useMe } from '$lib/queries';
	import { errMessage } from '$lib/api';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';

	let { active = false }: { active?: boolean } = $props();

	const me = useMe();
	const isAdmin = $derived(me.data?.role === 'admin');
	const roleLabel = $derived(
		me.data?.role === 'admin' ? m.set_profile_role_admin() : m.set_profile_role_user()
	);

	let revoking = $state(false);
	async function revokeKey() {
		const ok = await confirmDialog({
			title: m.set_profile_revoke_title(),
			message: m.set_profile_revoke_message(),
			confirmLabel: m.set_profile_revoke_confirm(),
			tone: 'danger'
		});
		if (!ok) return;
		revoking = true;
		try {
			await settingsApi.revokeMyKey();
			auth.markLoggedOut();
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			revoking = false;
		}
	}
</script>

<SettingSection id="profile" icon="◉" title={m.set_profile_title()}>
	<SettingGroup>
		<SettingRow label={m.set_profile_name_label()} help={m.set_profile_name_help()} selfLabelled>
			<Text size="sm" weight="semibold">{me.data?.user_name ?? '—'}</Text>
		</SettingRow>
		<SettingRow label={m.set_profile_role_label()} selfLabelled>
			<Text size="sm">{roleLabel}</Text>
		</SettingRow>
		<SettingRow
			label={m.set_profile_revoke_label()}
			help={me.data?.token_preview
				? m.set_profile_revoke_help({ key: me.data.token_preview })
				: m.set_profile_revoke_help_generic()}
			selfLabelled
		>
			<Button tone="danger" loading={revoking} onclick={revokeKey}>{m.set_profile_revoke_button()}</Button>
		</SettingRow>
	</SettingGroup>
</SettingSection>

<SecuritySection />

<SettingSection id="machines" icon="▥" title={m.set_my_machines_title()} description={m.set_my_machines_desc()}>
	<MachineAdaptersGroup />
	{#if active && me.data && !isAdmin}
		<div data-journey="page" data-journey-key="users"><AccessPanel /></div>
	{/if}
</SettingSection>

<SettingSection id="providers" icon="◎" title={m.set_providers_title()}>
	{#snippet descriptionSlot()}
		{m.set_providers_desc()}
		<Link href="/accounts">{m.set_providers_open_board()}</Link>
	{/snippet}
	{#if active}
		<div class="board"><AccountsPanel heading={false} /></div>
	{/if}
</SettingSection>

<style>
	.board {
		min-width: 0;
	}
</style>
