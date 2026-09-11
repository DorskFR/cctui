<script lang="ts">
	import { goto } from '$app/navigation';
	import { errMessage } from '$lib/api';
	import {
		useAccountActions,
		type OAuthAccount,
		type AccountProvider,
		type CreateAccount,
		type CreateProvider
	} from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { safeHref } from '$lib/safeHref';
	import { isStaticCredential, PROVIDER_KINDS, type ProviderKind } from '$lib/providers';
	import AccountAvatar from '$lib/components/molecules/AccountAvatar.svelte';
	import EmojiPicker from '$lib/components/molecules/EmojiPicker.svelte';
	import { isValidAccountEmoji } from '$lib/components/molecules/avatar';
	import { Button, Field, Input, Link, Modal, Select, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { availableKinds } from './account-editor.logic';

	let {
		rows,
		isAdmin = false,
		activeUsers = []
	}: {
		rows: OAuthAccount[];
		isAdmin?: boolean;
		/** Admin only: owners a new account may belong to. */
		activeUsers?: { id: string; name: string }[];
	} = $props();

	const actions = useAccountActions();
	let ownerId = $state('');
	$effect(() => {
		if (isAdmin && !ownerId && activeUsers.length) ownerId = activeUsers[0].id;
	});

	// Step one only: an identity plus the credential needed to reach the
	// provider. Everything tunable (models, aliases, limits, settings, env) is
	// edited afterwards on the account page and in the provider drawer, so this
	// modal stays a single short form.
	type EditorMode = 'create' | 'add-provider' | 'reauth';
	let editor = $state<{ mode: EditorMode; accountId?: string; providerId?: string } | null>(null);

	const editingAccount = $derived(
		editor?.accountId ? rows.find((a) => a.id === editor?.accountId) : undefined
	);

	let name = $state('');
	let emoji = $state('');
	let provider = $state<ProviderKind>('anthropic');
	let refreshToken = $state('');
	let baseUrl = $state('');
	let credential = $state('');
	let authScheme = $state<'bearer' | 'api_key'>('bearer');

	// Fireworks shares the static-credential shape but its base URL is an
	// optional override of a built-in upstream, so it is not required.
	const isFireworks = $derived(provider === 'fireworks');
	const isCompatible = $derived(isStaticCredential(provider));

	// "Sign in with Claude" / "Sign in with ChatGPT" OAuth flow state.
	let oauthNonce = $state<string | null>(null);
	let oauthCode = $state('');
	let oauthBusy = $state(false);
	let showAdvanced = $state(false);
	// OAuth attach target: finish the flow as a provider under this existing
	// account instead of creating a new identity.
	let oauthAttachAccountId = $state<string | null>(null);

	function resetForm() {
		name = '';
		emoji = '';
		provider = 'anthropic';
		refreshToken = '';
		baseUrl = '';
		credential = '';
		authScheme = 'bearer';
		oauthNonce = null;
		oauthCode = '';
		oauthBusy = false;
		showAdvanced = false;
		oauthAttachAccountId = null;
	}

	async function startOAuthLogin() {
		if (isAdmin && !ownerId && !oauthAttachAccountId) {
			toasts.error(m.accounts_err_pick_owner());
			return;
		}
		oauthBusy = true;
		try {
			const r = await actions.oauthStart(
				provider,
				isAdmin ? ownerId : undefined,
				oauthAttachAccountId ?? undefined
			);
			oauthNonce = r.nonce;
			const authorizeUrl = safeHref(r.authorize_url);
			if (!authorizeUrl) throw new Error(m.common_error());
			window.open(authorizeUrl, '_blank', 'noopener');
			toasts.ok(
				provider === 'openai' ? m.accounts_oauth_opened_chatgpt() : m.accounts_oauth_opened_claude()
			);
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			oauthBusy = false;
		}
	}

	// Claude sends `code` (the code#state pair); Codex sends `callback_url` (the
	// full localhost:1455 URL). With an attach target the credential lands under
	// that account and the name is ignored server-side.
	async function finishOAuthLogin() {
		if (!oauthAttachAccountId && !name.trim()) {
			toasts.error(m.accounts_err_name_required());
			return;
		}
		if (!oauthNonce || !oauthCode.trim()) {
			toasts.error(
				provider === 'openai' ? m.accounts_err_paste_url_first() : m.accounts_err_paste_code_first()
			);
			return;
		}
		oauthBusy = true;
		try {
			const acctName = name.trim() || editingAccount?.name || '';
			const created = await actions.oauthFinish(
				provider === 'openai'
					? { nonce: oauthNonce, name: acctName, callback_url: oauthCode.trim() }
					: { nonce: oauthNonce, name: acctName, code: oauthCode.trim() }
			);
			const attached = oauthAttachAccountId;
			toasts.ok(attached ? m.accounts_provider_added() : m.accounts_account_added());
			close();
			land(attached ?? created?.id);
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			oauthBusy = false;
		}
	}

	export function openCreate() {
		resetForm();
		editor = { mode: 'create' };
	}

	export function openAddProvider(a: OAuthAccount) {
		resetForm();
		editor = { mode: 'add-provider', accountId: a.id };
		provider = availableKinds(a)[0] ?? 'anthropic';
		ownerId = a.user_id;
		oauthAttachAccountId = a.id;
	}

	// Reauthenticate a flagged provider: open the sign-in block and kick the
	// authorize leg. The pasted code refreshes the same-family credential in
	// place and clears `needs_reauth`.
	export function reauth(a: OAuthAccount, p: AccountProvider) {
		resetForm();
		editor = { mode: 'reauth', accountId: a.id, providerId: p.id };
		provider = p.provider as ProviderKind;
		ownerId = a.user_id;
		oauthAttachAccountId = a.id;
		startOAuthLogin();
	}

	function close() {
		editor = null;
	}

	// Everything past the credential is configured on the account page, so send
	// the operator there rather than leaving them on the board.
	function land(accountId: string | undefined) {
		if (accountId) goto(`/accounts/${accountId}`);
	}

	function credentialSpec(): Partial<CreateProvider> | null {
		if (isFireworks) {
			return {
				auth_scheme: authScheme,
				...(baseUrl.trim() ? { base_url: baseUrl.trim() } : {}),
				...(credential.trim() ? { access_token: credential.trim() } : {})
			};
		}
		if (isCompatible) {
			if (!baseUrl.trim()) {
				toasts.error(m.accounts_err_base_url_required());
				return null;
			}
			return {
				base_url: baseUrl.trim(),
				auth_scheme: authScheme,
				...(credential.trim() ? { access_token: credential.trim() } : {})
			};
		}
		if (!refreshToken.trim()) {
			toasts.error(m.accounts_err_refresh_token_required());
			return null;
		}
		return { refresh_token: refreshToken.trim() };
	}

	async function save() {
		const mode = editor?.mode;
		try {
			const cred = credentialSpec();
			if (!cred) return;
			if (mode === 'add-provider' && editor?.accountId) {
				const accountId = editor.accountId;
				await actions.addProvider(accountId, { provider, ...cred } as CreateProvider);
				toasts.ok(m.accounts_provider_added());
				close();
				land(accountId);
				return;
			}
			if (!name.trim()) {
				toasts.error(m.accounts_err_name_required());
				return;
			}
			if (isAdmin && !ownerId) {
				toasts.error(m.accounts_err_pick_owner());
				return;
			}
			if (!isValidAccountEmoji(emoji)) {
				toasts.error(m.account_emoji_invalid());
				return;
			}
			const body = {
				name: name.trim(),
				...(emoji.trim() ? { emoji: emoji.trim() } : {}),
				provider,
				...cred,
				...(isAdmin ? { user_id: ownerId } : {})
			} as CreateAccount;
			const created = await actions.create(body);
			toasts.ok(m.accounts_account_added());
			close();
			land(created?.id);
		} catch (e) {
			toasts.error(errMessage(e));
		}
	}

	// Native OAuth flows save via the pasted-code exchange instead.
	const oauthSaves = $derived(
		editor !== null &&
			(editor.mode === 'reauth' ||
				(!isCompatible && oauthNonce !== null && !showAdvanced))
	);

	const modalTitle = $derived(
		editor?.mode === 'create'
			? m.accounts_modal_new_account()
			: editor?.mode === 'add-provider'
				? m.accounts_modal_add_provider({ name: editingAccount?.name ?? '' })
				: m.accounts_modal_reauth()
	);
</script>

{#if editor !== null}
	<Modal title={modalTitle} onclose={close} size="md" resizeKey="account-editor">
		{#snippet body()}
			<div class="editor-body">
				{#if editor?.mode === 'create'}
					<Field label={m.accounts_field_name()}>
						<Input bind:value={name} placeholder={m.accounts_field_name_placeholder()} />
					</Field>
					<Field label={m.account_emoji_label()}>
						<div class="emoji-field">
							<AccountAvatar {emoji} {name} id={name} size={24} />
							<EmojiPicker value={emoji} onselect={(v) => (emoji = v)} />
							<Input
								bind:value={emoji}
								placeholder={m.account_emoji_placeholder()}
								maxlength={16}
								aria-label={m.account_emoji_label()}
								style="max-width: 8rem"
							/>
						</div>
						{#if !isValidAccountEmoji(emoji)}
							<Text tone="danger" size="xs">{m.account_emoji_invalid()}</Text>
						{/if}
					</Field>
					{#if isAdmin}
						<Field label={m.accounts_field_owner()}>
							<Select bind:value={ownerId} aria-label={m.accounts_field_owner()}>
								{#each activeUsers as u (u.id)}
									<option value={u.id}>{u.name}</option>
								{/each}
							</Select>
						</Field>
					{/if}
				{/if}

				{#if editor?.mode === 'create' || editor?.mode === 'add-provider'}
					<Field label={m.accounts_field_provider()}>
						<Select
							bind:value={provider}
							aria-label={m.accounts_field_provider()}
							onchange={() => {
								oauthNonce = null;
								oauthCode = '';
							}}
						>
							{#each editor?.mode === 'add-provider' && editingAccount ? availableKinds(editingAccount) : PROVIDER_KINDS.map((k) => k.value) as v (v)}
								<option value={v}>{PROVIDER_KINDS.find((k) => k.value === v)?.label ?? v}</option>
							{/each}
						</Select>
					</Field>
				{/if}

				{#if isFireworks}
					<Field label={m.accounts_field_credential()}>
						<Input type="password" bind:value={credential} placeholder="fw_..." />
					</Field>
					<Field label={m.accounts_field_base_url()}>
						<Input bind:value={baseUrl} placeholder="https://api.fireworks.ai/inference/v1" />
					</Field>
				{:else if isCompatible}
					<Field label={m.accounts_field_base_url()}>
						<Input bind:value={baseUrl} placeholder="https://litellm.example/v1" />
					</Field>
					<Field label={m.accounts_field_auth_scheme()}>
						<Select bind:value={authScheme} aria-label={m.accounts_field_auth_scheme()}>
							<option value="bearer">{m.accounts_auth_bearer()}</option>
							<option value="api_key">{m.accounts_auth_api_key()}</option>
						</Select>
					</Field>
					<Field label={m.accounts_field_credential()}>
						<Input
							type="password"
							bind:value={credential}
							placeholder={m.accounts_placeholder_credential()}
						/>
					</Field>
				{:else}
					{#if !oauthNonce}
						<Button
							variant="primary"
							style="align-self: flex-start"
							disabled={oauthBusy}
							onclick={startOAuthLogin}
						>
							{oauthBusy
								? m.accounts_oauth_opening()
								: provider === 'openai'
									? m.accounts_signin_chatgpt()
									: m.accounts_signin_claude()}
						</Button>
					{:else}
						<Field
							label={provider === 'openai'
								? m.accounts_oauth_url_label()
								: m.accounts_oauth_code_label()}
						>
							<Input
								bind:value={oauthCode}
								placeholder={provider === 'openai'
									? m.accounts_oauth_url_placeholder()
									: m.accounts_oauth_code_placeholder()}
							/>
						</Field>
						{#if provider === 'openai'}
							<Text as="p" tone="muted" size="sm">{m.accounts_oauth_localhost_note()}</Text>
						{/if}
						<Text as="p" tone="muted" size="sm">
							{provider === 'openai'
								? m.accounts_oauth_missing_url()
								: m.accounts_oauth_missing_code()}
							<Link onclick={startOAuthLogin}>
								{provider === 'openai'
									? m.accounts_oauth_reopen_chatgpt()
									: m.accounts_oauth_reopen_claude()}
							</Link>
						</Text>
					{/if}
					{#if editor?.mode !== 'reauth'}
						<details bind:open={showAdvanced} class="adv">
							<summary>
								<Text tone="muted" size="sm">{m.accounts_adv_refresh_summary()}</Text>
							</summary>
							<div class="adv-fld">
								<Field label={m.accounts_refresh_token_label()}>
									<Input
										type="password"
										bind:value={refreshToken}
										placeholder={m.accounts_refresh_token_placeholder()}
									/>
								</Field>
							</div>
						</details>
					{/if}
				{/if}

				{#if editor?.mode !== 'reauth'}
					<Text as="p" tone="faint" size="xs" measure="60ch">{m.accounts_next_step_hint()}</Text>
				{/if}
			</div>
		{/snippet}
		{#snippet footer()}
			<div class="spacer"></div>
			<Button onclick={close}>{m.common_cancel()}</Button>
			{#if oauthSaves}
				<Button variant="primary" disabled={oauthBusy} onclick={finishOAuthLogin}>
					{m.common_save()}
				</Button>
			{:else}
				<Button variant="primary" onclick={save}>{m.common_save()}</Button>
			{/if}
		{/snippet}
	</Modal>
{/if}

<style>
	.editor-body {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.emoji-field {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.adv summary {
		cursor: pointer;
	}
	.adv-fld {
		margin-top: var(--sp-2);
	}
	.spacer {
		flex: 1;
	}
</style>
