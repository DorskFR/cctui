<script lang="ts">
	// Settings › Security: the caller's own WebAuthn credentials (list, enrol,
	// test, revoke) plus the one server-wide knob (admin) that decides whether
	// the login screen reads the key on its own. The list is loaded on demand
	// rather than through the query cache: it changes only from this screen.
	import { Button, Icon, Input, Switch, Text } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import { endpoints } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { auth } from '$lib/auth.svelte';
	import { PasskeyAborted, createPasskey, getAssertion, passkeysSupported } from '$lib/passkeys';
	import type { PasskeyConfig } from '@bindings/PasskeyConfig';
	import type { PasskeyRow } from '@bindings/PasskeyRow';
	import type { JsonValue } from '@bindings/serde_json/JsonValue';
	import { m } from '$lib/paraglide/messages';

	let { isAdmin }: { isAdmin: boolean } = $props();

	let passkeyCfg = $state<PasskeyConfig | null>(null);
	let passkeyList = $state<PasskeyRow[]>([]);
	let passkeyLabel = $state('');
	let passkeyBusy = $state(false);
	let passkeyTesting = $state(false);
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
		try {
			const challenge = await endpoints.passkeyRegisterStart();
			const { credential, discoverable } = await createPasskey(
				challenge.options as Record<string, unknown>
			);
			await endpoints.passkeyRegisterFinish({
				challenge_id: challenge.challenge_id,
				label: passkeyLabel.trim() || null,
				// The credential is the W3C JSON blob; the binding types it as
				// `JsonValue` because the server hands it straight to webauthn-rs.
				credential: credential as JsonValue,
				discoverable
			});
			passkeyLabel = '';
			await loadPasskeys();
			toasts.ok(m.settings_passkeys_enrolled());
		} catch (e) {
			if (!(e instanceof PasskeyAborted)) toasts.error(e instanceof Error ? e.message : String(e));
		} finally {
			passkeyBusy = false;
		}
	}

	async function testPasskey() {
		if (passkeyTesting) return;
		passkeyTesting = true;
		try {
			const challenge = await endpoints.passkeyTestStart();
			const credential = await getAssertion(challenge.options as Record<string, unknown>);
			const res = await endpoints.passkeyTestFinish({
				challenge_id: challenge.challenge_id,
				credential: credential as JsonValue
			});
			toasts.ok(m.settings_passkeys_test_ok({ label: res.label }));
		} catch (e) {
			if (!(e instanceof PasskeyAborted)) toasts.error(e instanceof Error ? e.message : String(e));
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

	async function setPasskeyAutoPrompt(on: boolean) {
		try {
			await endpoints.setPasskeyAutoPrompt(on);
			if (passkeyCfg) passkeyCfg = { ...passkeyCfg, auto_prompt: on };
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		}
	}

	$effect(() => {
		void loadPasskeys();
	});

	function keyMeta(key: PasskeyRow): string {
		const parts = [m.settings_passkeys_added({ date: new Date(key.created_at).toLocaleDateString() })];
		parts.push(
			key.last_used_at
				? m.settings_passkeys_last_used({ date: new Date(key.last_used_at).toLocaleString() })
				: m.settings_passkeys_never_used()
		);
		return parts.join(' · ');
	}
</script>

<SettingSection
	id="security"
	icon="⚿"
	title={m.settings_nav_security()}
	description={passkeysUsable ? m.settings_passkeys_help() : m.settings_passkeys_unavailable()}
>
	{#if passkeysUsable}
		<SettingGroup title={m.settings_passkeys_group()}>
			{#each passkeyList as key (key.id)}
				<div class="key" data-setting-row>
					<span class="kicon"><Icon name="lock" size={16} /></span>
					<div class="kbody">
						<Text weight="semibold" size="sm" as="div">{key.label}</Text>
						<Text size="xs" tone="faint" as="div">{keyMeta(key)}</Text>
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
		</SettingGroup>
		{#if isAdmin}
			<SettingGroup>
				<SettingRow
					label={m.settings_passkeys_auto_prompt_label()}
					help={m.settings_passkeys_auto_prompt_help()}
					server
					admin
				>
					<Switch
						checked={passkeyCfg?.auto_prompt === true}
						label={m.settings_passkeys_auto_prompt_label()}
						onclick={() => setPasskeyAutoPrompt(passkeyCfg?.auto_prompt !== true)}
					/>
				</SettingRow>
			</SettingGroup>
		{/if}
	{/if}
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
	.enrol {
		flex-wrap: wrap;
	}
</style>
