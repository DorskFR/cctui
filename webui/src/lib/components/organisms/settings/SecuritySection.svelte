<script lang="ts">
	// Settings › Accounts › Sign-in & security: the caller's own WebAuthn
	// credentials (list, enrol, test, revoke) and device approvals. The
	// server-wide sign-in knob lives under Instance. The list is loaded on
	// demand rather than through the query cache: it changes only from here.
	import { Button, Icon, Input, Text, Timestamp } from '@dorsk/tsumikit';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import SettingSection from '#lib/components/molecules/SettingSection.svelte';
	import { hasFeature } from '#lib/features.svelte.ts';
	import { endpoints } from '#lib/queries/index.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { auth } from '#lib/auth.svelte.ts';
	import { needsSignInAgain, passkeyDismissed } from '#lib/passkeys.ts';
	import {
		createPasskey,
		getPasskey,
		passkeysSupported,
		type CreationOptionsJSON,
		type RequestOptionsJSON
	} from '@dorsk/kagi';
	import type { PasskeyConfig } from '@bindings/PasskeyConfig';
	import type { PasskeyRow } from '@bindings/PasskeyRow';
	import { m } from '#lib/paraglide/messages.js';

	let passkeyCfg = $state<PasskeyConfig | null>(null);
	let passkeyList = $state<PasskeyRow[]>([]);
	let passkeyLabel = $state('');
	let passkeyBusy = $state(false);
	let passkeyTesting = $state(false);
	let signInAgain = $state(false);
	const passkeysUsable = $derived(passkeysSupported() && !!passkeyCfg?.available);

	async function loadPasskeys() {
		passkeyCfg = await auth.passkeyConfig();
		if (!passkeyCfg?.available) return;
		try {
			passkeyList = (await endpoints.passkeys()).passkeys;
		} catch {
			// A server too old to know the route simply has no passkeys to show.
			passkeyList = [];
		}
	}

	async function enrollPasskey() {
		if (passkeyBusy) return;
		passkeyBusy = true;
		signInAgain = false;
		try {
			const challenge = await endpoints.passkeyRegisterStart();
			const credential = await createPasskey(challenge.options as CreationOptionsJSON);
			await endpoints.passkeyRegisterFinish({
				ceremony: challenge.ceremony,
				label: passkeyLabel.trim() || null,
				credential
			});
			passkeyLabel = '';
			await loadPasskeys();
			toasts.ok(m.settings_passkeys_enrolled());
		} catch (e) {
			if (needsSignInAgain(e)) signInAgain = true;
			else if (!passkeyDismissed(e)) toasts.error(e instanceof Error ? e.message : String(e));
		} finally {
			passkeyBusy = false;
		}
	}

	async function testPasskey() {
		if (passkeyTesting) return;
		passkeyTesting = true;
		try {
			const challenge = await endpoints.passkeyTestStart();
			const credential = await getPasskey(challenge.options as RequestOptionsJSON);
			if (!credential) return;
			const res = await endpoints.passkeyTestFinish({ ceremony: challenge.ceremony, credential });
			toasts.ok(m.settings_passkeys_test_ok({ label: res.label }));
		} catch (e) {
			if (!passkeyDismissed(e)) toasts.error(e instanceof Error ? e.message : String(e));
		} finally {
			passkeyTesting = false;
		}
	}

	async function revokePasskey(id: string) {
		try {
			await endpoints.revokePasskey(id);
			await loadPasskeys();
			toasts.ok(m.settings_passkeys_revoked());
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		}
	}

	$effect(() => {
		if (hasFeature('passkeys')) void loadPasskeys();
	});

</script>

<SettingSection
	id="security"
	icon="⚿"
	title={m.settings_sec_security()}
	description={passkeysUsable ? m.settings_passkeys_help() : m.settings_passkeys_unavailable()}
>
	{#if passkeysUsable}
		<SettingGroup title={m.settings_passkeys_group()}>
			{#each passkeyList as key (key.id)}
				<div class="key" data-setting-row>
					<span class="kicon"><Icon name="lock" size={16} /></span>
					<div class="kbody">
						<Text weight="semibold" size="sm" as="div">{key.label}</Text>
						<Text size="xs" tone="faint" as="div">
							{m.settings_passkeys_added_label()} <Timestamp value={key.created_at} mode="date" size="xs" tone="inherit" />
							·
							{#if key.last_used_at}
								{m.settings_passkeys_last_used_label()} <Timestamp value={key.last_used_at} mode="relative" size="xs" tone="inherit" />
							{:else}
								{m.settings_passkeys_never_used()}
							{/if}
						</Text>
						{#if !key.discoverable}
							<Text size="xs" tone="danger" as="div">{m.settings_passkeys_not_discoverable()}</Text>
						{/if}
					</div>
					<Button size="sm" variant="ghost" hoverDanger onclick={() => revokePasskey(key.id)}>
						{m.settings_passkeys_revoke()}
					</Button>
				</div>
			{/each}
			<div class="enrol" data-setting-row>
				<Input
					bind:value={passkeyLabel}
					maxlength={64}
					grow
					placeholder={m.settings_passkeys_name_placeholder()}
					aria-label={m.settings_passkeys_add_label()}
					onkeydown={(e: KeyboardEvent) => {
						if (e.key === 'Enter' && !passkeyBusy) enrollPasskey();
					}}
				/>
				<Button variant="primary" disabled={passkeyBusy} onclick={enrollPasskey}>
					{m.settings_passkeys_add()}
				</Button>
				{#if passkeyList.length > 0}
					<Button
						variant="ghost"
						disabled={passkeyTesting}
						title={m.settings_passkeys_test_help()}
						onclick={testPasskey}
					>
						{m.settings_passkeys_test()}
					</Button>
				{/if}
			</div>
			{#if signInAgain}
				<div class="stepup" role="alert" data-testid="passkey-sign-in-again">
					<Text size="sm" tone="danger" as="div">{m.settings_passkeys_sign_in_again()}</Text>
					<Button size="sm" variant="primary" onclick={() => auth.logout()}>
						{m.settings_passkeys_sign_in_again_action()}
					</Button>
				</div>
			{/if}
		</SettingGroup>
	{/if}
	<SettingGroup>
		<SettingRow label={m.settings_device_approvals_label()} help={m.settings_device_approvals_help()} selfLabelled>
			<Button size="sm" variant="ghost" as="a" href="/device">{m.settings_device_approvals_open()}</Button>
		</SettingRow>
	</SettingGroup>
</SettingSection>

<style>
	.key,
	.enrol {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		padding: var(--sp-3) var(--sp-4);
		border-top: 1px solid var(--border);
	}
	.key:first-child {
		border-top: 0;
	}
	.kicon {
		width: 2rem;
		height: 2rem;
		border-radius: var(--r-md);
		background: var(--surface);
		display: inline-flex;
		align-items: center;
		justify-content: center;
		color: var(--text-muted);
		flex: none;
	}
	.kbody {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.enrol,
	.stepup {
		flex-wrap: wrap;
	}
	.stepup {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-3);
		padding: var(--sp-3) var(--sp-4);
		border-top: 1px solid var(--border);
	}
</style>
