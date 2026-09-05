<script lang="ts">
	import { errMessage } from '$lib/api';
	import {
		useAccountActions,
		useAccountUsage,
		type OAuthAccount,
		type AccountModel,
		type AccountProvider,
		type CreateAccount,
		type CreateProvider,
		type UpdateAccount
	} from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { safeHref } from '$lib/safeHref';
	import { isStaticCredential, PROVIDER_KINDS, type ProviderKind } from '$lib/providers';
	import SoftLimit from '$lib/components/molecules/SoftLimit.svelte';
	import AccountAvatar from '$lib/components/molecules/AccountAvatar.svelte';
	import { isValidAccountEmoji } from '$lib/components/molecules/avatar';
	import { editorWindowKeys, isUsdKey } from '$lib/components/molecules/usage-windows';
	import FireworksProviderEditor from '$lib/components/organisms/FireworksProviderEditor.svelte';
	import FreeFormEnvEditor from '$lib/components/organisms/FreeFormEnvEditor.svelte';
	import ProviderDrawer from './provider-drawer/ProviderDrawer.svelte';
	import { Button, Field, Input, Link, Modal, Select, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import {
		aliasObject,
		availableKinds,
		buildRateLimits,
		buildSoftLimits,
		envObject,
		fwModelList,
		modelList
	} from './account-editor.logic';

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

	// Editor state. One modal, four modes:
	//   create        — new identity + its first provider credential
	//   add-provider  — attach a credential to an existing identity
	//   edit-account  — identity fields: name + write-only extra env
	//   reauth        — refresh a rejected credential through the sign-in block
	// Editing an existing credential happens in the provider drawer instead.
	type EditorMode = 'create' | 'add-provider' | 'edit-account' | 'reauth';
	let editor = $state<{ mode: EditorMode; accountId?: string; providerId?: string } | null>(null);
	let drawer = $state<{ accountId: string; providerId: string } | null>(null);

	const editingAccount = $derived(
		editor?.accountId ? rows.find((a) => a.id === editor?.accountId) : undefined
	);
	const drawerAccount = $derived(drawer ? rows.find((a) => a.id === drawer?.accountId) : undefined);
	const drawerProvider = $derived(
		drawerAccount?.providers.find((p) => p.id === drawer?.providerId)
	);

	let name = $state('');
	let emoji = $state('');
	let provider = $state<ProviderKind>('anthropic');
	let refreshToken = $state('');
	// Compatible-endpoint fields: base URL, a static credential, the
	// auth scheme, and a tiny model-list editor (model code + display label).
	let baseUrl = $state('');
	let credential = $state('');
	// `keep` is an edit-only sentinel: leave the stored scheme untouched (it
	// is never read back). Create always picks bearer/api_key.
	let authScheme = $state<'bearer' | 'api_key' | 'keep'>('bearer');
	let modelRows = $state<{ model: string; label: string }[]>([{ model: '', label: '' }]);
	// Per-provider model alias map: logical name → concrete model code,
	// e.g. opus → claude-opus-4-8[1m]. Resolved server-side at spawn.
	let aliasRows = $state<{ alias: string; model: string }[]>([]);
	// Per-provider soft limits: cap cctui's own share of each usage
	// window, keyed by canonical window id. Edited per window via the reusable
	// SoftLimit component; empty inputs = no cap/bypass on that window.
	let softEdits = $state<
		Record<string, { cap: number | null; capUsd: number | null; bypass: number | null }>
	>({});
	// Gateway RPM/TPM ceilings shared across the account's concurrent sessions;
	// empty inputs ⇒ that dimension unlimited.
	let rateEdits = $state<{ rpm: number | null; tpm: number | null }>({ rpm: null, tpm: null });
	// Fireworks shares the static-credential shape (no OAuth) but keeps its own
	// editor: gateway settings + a priced model catalog instead of a bare model
	// list, and its base URL is an optional override of a built-in upstream.
	const isFireworks = $derived(provider === 'fireworks');
	const isCompatible = $derived(isStaticCredential(provider));
	// Left empty on create: the server seeds the default settings + catalog, so
	// the seed lives in exactly one place.
	let fwSettings = $state<Record<string, unknown>>({});
	let fwModels = $state<AccountModel[]>([]);
	// Window keys to offer on a fresh credential: the baseline pair, so caps can
	// be set before any usage is reported.
	const editorRows = $derived(
		editor?.mode === 'create' || editor?.mode === 'add-provider'
			? editorWindowKeys([], null, isFireworks ? 'fireworks' : null)
			: []
	);
	// Ensure every offered key has an edit slot (seeded null; open* seeds configured).
	$effect(() => {
		for (const { key } of editorRows) {
			if (!(key in softEdits)) softEdits[key] = { cap: null, capUsd: null, bypass: null };
		}
	});

	// `envRows` feed the identity's write-only env_json (never read back, so they
	// start empty on edit); `replaceEnv` gates whether env_json is sent at all.
	let acctEnvRows = $state<{ name: string; value: string }[]>([]);
	let acctReplaceEnv = $state(false);
	let acctEnvRemove = $state<string[]>([]);

	// "Sign in with Claude" OAuth flow state.
	let oauthNonce = $state<string | null>(null);
	let oauthCode = $state('');
	let oauthBusy = $state(false);
	let showAdvanced = $state(false);
	// OAuth attach target: when adding/reauthenticating, finish the flow
	// as a provider under this existing account instead of creating a new identity.
	let oauthAttachAccountId = $state<string | null>(null);

	function resetForm() {
		name = '';
		emoji = '';
		provider = 'anthropic';
		refreshToken = '';
		baseUrl = '';
		credential = '';
		authScheme = 'bearer';
		modelRows = [{ model: '', label: '' }];
		aliasRows = [];
		softEdits = {};
		rateEdits = { rpm: null, tpm: null };
		oauthNonce = null;
		oauthCode = '';
		oauthBusy = false;
		showAdvanced = false;
		oauthAttachAccountId = null;
		acctEnvRows = [];
		acctReplaceEnv = false;
		acctEnvRemove = [];
		fwSettings = {};
		fwModels = [];
	}

	// Start the authorize leg: ask the server for an authorize URL, open it in a
	// new tab, and reveal the paste field. Works for both Claude (anthropic) and
	// "Sign in with ChatGPT" for Codex (openai).
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
				oauthAttachAccountId ?? undefined,
			);
			oauthNonce = r.nonce;
			const authorizeUrl = safeHref(r.authorize_url);
			if (!authorizeUrl) throw new Error(m.common_error());
			window.open(authorizeUrl, '_blank', 'noopener');
			if (provider === 'openai') {
				toasts.ok(m.accounts_oauth_opened_chatgpt());
			} else {
				toasts.ok(m.accounts_oauth_opened_claude());
			}
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			oauthBusy = false;
		}
	}

	// Finish: exchange the pasted code/callback URL for tokens and create the
	// account/provider. Claude sends `code` (the code#state pair); Codex sends
	// `callback_url` (the full localhost:1455 URL from the address bar). With an
	// attach target the credential lands under that account and the
	// name is ignored server-side.
	async function finishOAuthLogin() {
		if (!oauthAttachAccountId && !name.trim()) {
			toasts.error(m.accounts_err_name_required());
			return;
		}
		if (!oauthNonce || !oauthCode.trim()) {
			toasts.error(
				provider === 'openai'
					? m.accounts_err_paste_url_first()
					: m.accounts_err_paste_code_first(),
			);
			return;
		}
		oauthBusy = true;
		try {
			const acctName = name.trim() || editingAccount?.name || '';
			await actions.oauthFinish(
				provider === 'openai'
					? { nonce: oauthNonce, name: acctName, callback_url: oauthCode.trim() }
					: { nonce: oauthNonce, name: acctName, code: oauthCode.trim() },
			);
			toasts.ok(oauthAttachAccountId ? m.accounts_provider_added() : m.accounts_account_added());
			close();
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
		// The native OAuth flows attach via oauth/start's account_id.
		oauthAttachAccountId = a.id;
	}

	export function openEditAccount(a: OAuthAccount) {
		resetForm();
		editor = { mode: 'edit-account', accountId: a.id };
		name = a.name;
		emoji = a.emoji ?? '';
		// env_json is write-only: rows start empty; editing them flips replaceEnv.
	}

	export function openEditProvider(a: OAuthAccount, p: AccountProvider) {
		drawer = { accountId: a.id, providerId: p.id };
	}

	// Reauthenticate a flagged provider: open the sign-in block and kick the
	// authorize leg. The pasted code is exchanged by finishOAuthLogin, which
	// refreshes the same-family credential in place and clears `needs_reauth`.
	export function reauth(a: OAuthAccount, p: AccountProvider) {
		resetForm();
		editor = { mode: 'reauth', accountId: a.id, providerId: p.id };
		provider = p.provider as ProviderKind;
		ownerId = a.user_id;
		// Attach the refreshed credential to THIS account rather than
		// creating a new identity from the name.
		oauthAttachAccountId = a.id;
		startOAuthLogin();
	}

	function close() {
		editor = null;
	}

	async function save() {
		const mode = editor?.mode;
		const model_aliases = aliasObject(aliasRows);
		try {
			if (mode === 'edit-account' && editor?.accountId) {
				if (!name.trim()) {
					toasts.error(m.accounts_err_name_required());
					return;
				}
				if (!isValidAccountEmoji(emoji)) {
					toasts.error(m.account_emoji_invalid());
					return;
				}
				const identity: UpdateAccount = { name: name.trim(), emoji: emoji.trim() };
				if (acctReplaceEnv) identity.env_json = envObject(acctEnvRows);
				else if (acctEnvRemove.length) identity.env_remove = acctEnvRemove;
				await actions.update(editor.accountId, identity);
				toasts.ok(m.accounts_account_updated());
			} else if (mode === 'add-provider' && editor?.accountId) {
				// Native OAuth adds go through finishOAuthLogin instead; this path is
				// the compatible-endpoint / pasted-refresh-token attach.
				const spec: CreateProvider = {
					provider,
					...(Object.keys(model_aliases).length ? { model_aliases } : {}),
					soft_limits: buildSoftLimits(softEdits),
					rate_limits: buildRateLimits(rateEdits)
				};
				if (isFireworks) {
					spec.auth_scheme = authScheme === 'keep' ? 'bearer' : authScheme;
					if (baseUrl.trim()) spec.base_url = baseUrl.trim();
					if (credential.trim()) spec.access_token = credential.trim();
					const models = fwModelList(fwModels);
					if (models.length) spec.models = models;
					if (Object.keys(fwSettings).length) spec.provider_settings = fwSettings;
				} else if (isCompatible) {
					if (!baseUrl.trim()) {
						toasts.error(m.accounts_err_base_url_required());
						return;
					}
					spec.base_url = baseUrl.trim();
					spec.auth_scheme = authScheme === 'keep' ? 'bearer' : authScheme;
					if (credential.trim()) spec.access_token = credential.trim();
					const models = modelList(modelRows);
					if (models.length) spec.models = models;
				} else {
					if (!refreshToken.trim()) {
						toasts.error(m.accounts_err_refresh_token_required());
						return;
					}
					spec.refresh_token = refreshToken.trim();
				}
				await actions.addProvider(editor.accountId, spec);
				toasts.ok(m.accounts_provider_added());
			} else {
				// create: identity + first credential in one call.
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
				const emojiField = emoji.trim() ? { emoji: emoji.trim() } : {};
				let body: CreateAccount;
				if (isFireworks) {
					const models = fwModelList(fwModels);
					body = {
						name: name.trim(),
						...emojiField,
						provider,
						auth_scheme: authScheme === 'keep' ? 'bearer' : authScheme,
						...(baseUrl.trim() ? { base_url: baseUrl.trim() } : {}),
						...(credential.trim() ? { access_token: credential.trim() } : {}),
						...(models.length ? { models } : {}),
						...(Object.keys(fwSettings).length ? { provider_settings: fwSettings } : {}),
						...(Object.keys(model_aliases).length ? { model_aliases } : {}),
						soft_limits: buildSoftLimits(softEdits),
						rate_limits: buildRateLimits(rateEdits),
						...(isAdmin ? { user_id: ownerId } : {})
					};
				} else if (isCompatible) {
					if (!baseUrl.trim()) {
						toasts.error(m.accounts_err_base_url_required());
						return;
					}
					const models = modelList(modelRows);
					body = {
						name: name.trim(),
						...emojiField,
						provider,
						base_url: baseUrl.trim(),
						auth_scheme: authScheme === 'keep' ? 'bearer' : authScheme,
						...(credential.trim() ? { access_token: credential.trim() } : {}),
						...(models.length ? { models } : {}),
						...(Object.keys(model_aliases).length ? { model_aliases } : {}),
						soft_limits: buildSoftLimits(softEdits),
						rate_limits: buildRateLimits(rateEdits),
						...(isAdmin ? { user_id: ownerId } : {}),
					};
				} else {
					if (!refreshToken.trim()) {
						toasts.error(m.accounts_err_refresh_token_required());
						return;
					}
					body = {
						name: name.trim(),
						...emojiField,
						provider,
						refresh_token: refreshToken.trim(),
						...(Object.keys(model_aliases).length ? { model_aliases } : {}),
						soft_limits: buildSoftLimits(softEdits),
						rate_limits: buildRateLimits(rateEdits),
						...(isAdmin ? { user_id: ownerId } : {}),
					};
				}
				await actions.create(body);
				toasts.ok(m.accounts_account_added());
			}
			close();
		} catch (e) {
			toasts.error(errMessage(e));
		}
	}

	// Native OAuth flows save via finishOAuthLogin (the pasted-code exchange).
	const oauthSaves = $derived(
		editor !== null &&
			(editor.mode === 'reauth' ||
				((editor.mode === 'create' || editor.mode === 'add-provider') &&
					!isCompatible &&
					oauthNonce !== null &&
					!showAdvanced))
	);

	const modalTitle = $derived(
		editor?.mode === 'create'
			? m.accounts_modal_new_account()
			: editor?.mode === 'add-provider'
				? m.accounts_modal_add_provider({ name: editingAccount?.name ?? '' })
				: editor?.mode === 'edit-account'
					? m.accounts_modal_edit_account()
					: m.accounts_modal_reauth()
	);
</script>

{#if editor !== null}
	<Modal title={modalTitle} onclose={close} size="lg" resizeKey="account-editor">
		{#snippet body()}
			<div class="editor-body">
				{#if editor?.mode === 'create' || editor?.mode === 'edit-account'}
					<Field label={m.accounts_field_name()}>
						<Input bind:value={name} placeholder={m.accounts_field_name_placeholder()} />
					</Field>
					<Field label={m.account_emoji_label()}>
						<div class="emoji-field">
							<AccountAvatar {emoji} {name} id={editor?.accountId ?? name} size={24} />
							<Input bind:value={emoji} placeholder={m.account_emoji_placeholder()} maxlength={16} style="max-width: 8rem" />
							<Button control onclick={() => (emoji = '')} disabled={!emoji}>
								{m.account_emoji_clear()}
							</Button>
						</div>
						{#if !isValidAccountEmoji(emoji)}
							<Text tone="danger" size="xs">{m.account_emoji_invalid()}</Text>
						{:else}
							<Text tone="faint" size="xs">{m.account_emoji_hint()}</Text>
						{/if}
					</Field>
				{/if}
				{#if editor?.mode === 'create' && isAdmin}
					<Field label={m.accounts_field_owner()}>
						<Select bind:value={ownerId}>
							{#each activeUsers as u (u.id)}
								<option value={u.id}>{u.name}</option>
							{/each}
						</Select>
					</Field>
				{/if}
				{#if editor?.mode === 'create' || editor?.mode === 'add-provider'}
					<Field label={m.accounts_field_provider()}>
						<Select
							bind:value={provider}
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

				{#if editor?.mode === 'edit-account'}
					<!-- Identity half: free-form write-only extra env lives
					     on the account; curated provider settings are edited per provider. -->
					<FreeFormEnvEditor
						bind:envRows={acctEnvRows}
						bind:replaceEnv={acctReplaceEnv}
						bind:envRemove={acctEnvRemove}
						storedNames={editingAccount?.env_names ?? []}
					/>
				{:else}
					{#if isFireworks}
						<!-- Fireworks: a static `fw_...` bearer. The base URL is an
						     optional override of the built-in upstream, so it is not
						     required; the gateway settings + priced model catalog live in
						     their own editor. -->
						<Field label={m.accounts_field_credential()}>
							<Input type="password" bind:value={credential} placeholder="fw_..." />
						</Field>
						<Field label={m.accounts_field_base_url()}>
							<Input bind:value={baseUrl} placeholder="https://api.fireworks.ai/inference/v1" />
						</Field>
						<FireworksProviderEditor bind:settings={fwSettings} bind:models={fwModels} />
					{:else if isCompatible}
							<!-- Compatible endpoint: base URL + a static credential + a model
						     list. No OAuth; the gateway forwards the credential and skips
						     refresh. -->
						<Field label={m.accounts_field_base_url()}>
							<Input bind:value={baseUrl} placeholder="https://litellm.example/v1" />
						</Field>
						<Field label={m.accounts_field_auth_scheme()}>
							<Select bind:value={authScheme}>
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
						<div class="models">
							<Text as="div" tone="muted" size="sm">{m.accounts_models_label()}</Text>
							{#each modelRows as row, i (i)}
								<div class="model-row">
									<Input
										bind:value={row.model}
										placeholder={m.accounts_placeholder_model_code()}
										aria-label={m.a11y_model_code()}
									/>
									<Input
										bind:value={row.label}
										placeholder={m.accounts_placeholder_model_label()}
										aria-label={m.a11y_model_label()}
									/>
									<Button
										variant="danger"
										onclick={() => (modelRows = modelRows.filter((_, j) => j !== i))}
										disabled={modelRows.length === 1}>✕</Button
									>
								</div>
							{/each}
							<Button
								onclick={() => (modelRows = [...modelRows, { model: '', label: '' }])}
								>{m.accounts_add_model()}</Button
							>
						</div>
					{:else if editor?.mode === 'create' || editor?.mode === 'add-provider' || editor?.mode === 'reauth'}
						<!-- Sign in with Claude / ChatGPT: authorize upstream, paste back.
						     Also shown when reauthenticating an existing provider. -->
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
							<Field label={provider === 'openai' ? m.accounts_oauth_url_label() : m.accounts_oauth_code_label()}>
								<Input
									bind:value={oauthCode}
									placeholder={provider === 'openai'
										? m.accounts_oauth_url_placeholder()
										: m.accounts_oauth_code_placeholder()}
								/>
							</Field>
							{#if provider === 'openai'}
								<Text as="p" tone="muted" size="sm">
									{m.accounts_oauth_localhost_note()}
								</Text>
							{/if}
							<Text as="p" tone="muted" size="sm">
								{provider === 'openai' ? m.accounts_oauth_missing_url() : m.accounts_oauth_missing_code()}
								<Link onclick={startOAuthLogin}
									>{provider === 'openai' ? m.accounts_oauth_reopen_chatgpt() : m.accounts_oauth_reopen_claude()}</Link
								>
							</Text>
						{/if}
						{#if editor?.mode !== 'reauth'}
							<details bind:open={showAdvanced} class="adv">
								<summary><Text tone="muted" size="sm">{m.accounts_adv_refresh_summary()}</Text></summary>
								<Field label={m.accounts_refresh_token_label()} class="adv-fld">
									<Input
										type="password"
										bind:value={refreshToken}
										placeholder={m.accounts_refresh_token_placeholder()}
									/>
								</Field>
							</details>
						{/if}
					{/if}

					<!-- Model aliases: logical name -> concrete model code,
					     resolved server-side at spawn; works for every provider. -->
					{#if isCompatible}
						<div class="models">
							<Text as="div" tone="muted" size="sm">{m.accounts_aliases_label()}</Text>
							<Text as="div" tone="faint" size="xs">
								{m.accounts_aliases_help()}
							</Text>
							{#each aliasRows as row, i (i)}
								<div class="model-row">
									<Input
										bind:value={row.alias}
										placeholder={m.accounts_placeholder_alias_name()}
										aria-label={m.a11y_alias_name()}
									/>
									<Input
										bind:value={row.model}
										placeholder={m.accounts_placeholder_alias_model()}
										aria-label={m.a11y_alias_model()}
									/>
									<Button
										variant="danger"
										onclick={() => (aliasRows = aliasRows.filter((_, j) => j !== i))}>✕</Button
									>
								</div>
							{/each}
							<Button onclick={() => (aliasRows = [...aliasRows, { alias: '', model: '' }])}
								>{m.accounts_add_alias()}</Button
							>
						</div>
					{/if}

					<!-- Soft limits: one reusable SoftLimit row per usage window
					     (baseline + observed + configured), so model-scoped windows appear
					     automatically. Works for anthropic (upstream usage API) and openai
					     (locally metered). -->
					{#if editor?.mode !== 'reauth' && (provider === 'anthropic' || provider === 'anthropic-compatible' || provider === 'openai' || isFireworks)}
						<div class="models">
							<Text as="div" tone="muted" size="sm">{m.accounts_soft_limits_label()}</Text>
							<Text as="div" tone="faint" size="xs">
								{m.accounts_soft_limits_help()}
							</Text>
							{#each editorRows as row (row.key)}
								{#if softEdits[row.key]}
									<SoftLimit
										label={row.label}
										editable
										usd={isUsdKey(row.key)}
										bind:cap={softEdits[row.key].cap}
										bind:capUsd={softEdits[row.key].capUsd}
										bind:bypass={softEdits[row.key].bypass}
									/>
								{/if}
							{/each}
						</div>
					{/if}

					<!-- Gateway rate limits: an account-wide RPM/TPM tier a
					     pay-per-token provider shares across every concurrent session,
					     throttled at the proxy. Blank = unlimited. -->
					{#if editor?.mode !== 'reauth'}
						<div class="models">
							<Text as="div" tone="muted" size="sm">{m.accounts_rate_limits_label()}</Text>
							<Text as="div" tone="faint" size="xs">{m.accounts_rate_limits_help()}</Text>
							<div class="rate-row">
								<Field label={m.accounts_rate_rpm_label()}>
									<Input type="number" min="0" step="1" bind:value={rateEdits.rpm} placeholder="e.g. 60" />
								</Field>
								<Field label={m.accounts_rate_tpm_label()}>
									<Input type="number" min="0" step="1" bind:value={rateEdits.tpm} placeholder="e.g. 90000" />
								</Field>
							</div>
						</div>
					{/if}
				{/if}
			</div>
		{/snippet}
		{#snippet footer()}
			<div class="spacer"></div>
			<Button onclick={close}>{m.common_cancel()}</Button>
			{#if oauthSaves}
				<Button variant="primary" disabled={oauthBusy} onclick={finishOAuthLogin}>{m.common_save()}</Button>
			{:else}
				<Button variant="primary" onclick={save}>{m.common_save()}</Button>
			{/if}
		{/snippet}
	</Modal>
{/if}

{#if drawer && drawerAccount && drawerProvider}
	<ProviderDrawer
		account={drawerAccount}
		provider={drawerProvider}
		accounts={rows}
		onclose={() => (drawer = null)}
	/>
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
	.models {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.model-row {
		display: grid;
		grid-template-columns: 1fr 1fr auto;
		gap: var(--sp-2);
		align-items: center;
	}
	.rate-row {
		display: grid;
		grid-template-columns: repeat(2, 1fr);
		gap: var(--sp-2);
		margin-top: var(--sp-1);
	}
	.adv :global(.adv-fld) {
		margin-top: var(--sp-2);
	}
</style>
