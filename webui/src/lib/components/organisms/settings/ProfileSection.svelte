<script lang="ts">
	import { Button, ConfirmModal, Text } from '@dorsk/tsumikit';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import SettingSection from '#lib/components/molecules/SettingSection.svelte';
	import AccessPanel from '#lib/components/organisms/access/AccessPanel.svelte';
	import { revokeCurrentKey, useMe } from '#lib/queries/index.ts';
	import { auth } from '#lib/auth.svelte.ts';
	import { errMessage } from '#lib/api.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';

	let { isAdmin = false, active = false }: { isAdmin?: boolean; active?: boolean } = $props();

	const me = useMe();
	let confirming = $state(false);
	let revoking = $state(false);

	async function revoke() {
		revoking = true;
		try {
			await revokeCurrentKey();
			await auth.logout();
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			revoking = false;
		}
	}
</script>

<SettingSection id="profile" icon="◉" title={m.settings_sec_profile()}>
	<SettingGroup>
		<SettingRow label={m.settings_profile_name_label()} help={m.settings_profile_name_help()} selfLabelled>
			<Text size="sm">{me.data?.user_name ?? '—'}</Text>
		</SettingRow>
		<SettingRow label={m.settings_profile_role_label()} selfLabelled>
			<Text size="sm" variant="code">{me.data?.role ?? '—'}</Text>
		</SettingRow>
		<SettingRow label={m.settings_profile_id_label()} selfLabelled>
			<Text size="sm" variant="code" tone="faint">{me.data?.user_id ?? '—'}</Text>
		</SettingRow>
		<SettingRow label={m.settings_profile_key_label()} help={m.settings_profile_key_help()} selfLabelled>
			<span class="key">
				<Text size="sm" variant="code" tone="faint">{me.data?.token_preview ?? ''}</Text>
				<Button
					size="sm"
					variant="ghost"
					hoverDanger
					disabled={revoking}
					data-testid="profile-revoke-key"
					onclick={() => (confirming = true)}
				>
					{m.settings_profile_key_revoke()}
				</Button>
			</span>
		</SettingRow>
	</SettingGroup>
	{#if !isAdmin && active}
		<AccessPanel />
	{/if}
</SettingSection>

<ConfirmModal
	bind:open={confirming}
	tone="danger"
	title={m.settings_profile_key_revoke_title()}
	message={m.settings_profile_key_revoke_message()}
	confirmLabel={m.settings_profile_key_revoke()}
	cancelLabel={m.common_cancel()}
	onconfirm={revoke}
/>

<style>
	.key {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		flex-wrap: wrap;
		justify-content: flex-end;
	}
</style>
