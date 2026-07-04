<script lang="ts">
	import {
		useAccounts,
		useAccountActions,
		useCapabilities,
		useMe,
		useUsers,
		type OAuthAccount,
		type AccountProvider,
		type CreateAccount,
		type CreateProvider,
		type UpdateAccount,
		type UpdateProvider,
	} from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { providerFamily, providerLabel, PROVIDER_KINDS, type ProviderKind } from '$lib/providers';
	import ProviderPanel from '$lib/components/molecules/ProviderPanel.svelte';
	import AccountShares from '$lib/components/molecules/AccountShares.svelte';
	import GithubConnectors from '$lib/components/organisms/GithubConnectors.svelte';
	import DispatchersPanel from '$lib/components/organisms/DispatchersPanel.svelte';
	import AccountSettingsEditor from '$lib/components/organisms/AccountSettingsEditor.svelte';
	import {
		AutoGrid,
		Button,
		Card,
		Cluster,
		Field,
		Heading,
		Input,
		Link,
		Modal,
		Select,
		Stack,
		Tabs,
		Text,
		Timestamp,
		type TabItem
	} from '@dorsk/tsumikit';

	const caps = useCapabilities();
	// Accounts is the single home for everything external (CCT-403): AI provider
	// accounts, GitHub connectors, and dispatchers. The Connectors tab only
	// appears when the integration is compiled in (`available`).
	let tab = $state('ai');
	const tabs = $derived<TabItem[]>([
		{ id: 'ai', label: 'AI accounts' },
		...($caps.data?.github.available ? [{ id: 'connectors', label: 'Connectors' }] : []),
		{ id: 'dispatchers', label: 'Dispatchers' }
	]);

	const accounts = useAccounts();
	const actions = useAccountActions();
	// Accounts are user-owned; the admin token has no user identity, so an
	// admin operator picks the owning user explicitly (CCT-251).
	const me = useMe();
	const isAdmin = $derived($me.data?.role === 'admin');
	const users = useUsers(() => isAdmin);
	const activeUsers = $derived(($users.data ?? []).filter((u) => !u.revoked_at));
	let ownerId = $state('');
	// Default the owner select to the first active user once loaded.
	$effect(() => {
		if (isAdmin && !ownerId && activeUsers.length) ownerId = activeUsers[0].id;
	});
	const guard = (p: Promise<unknown>) => p.catch((e: Error) => toasts.err(e.message));

	// ------------------------------------------------------------------------
	// Editor state (CCT-560). One modal, four modes:
	//   create        — new identity + its first provider credential
	//   add-provider  — attach a credential to an existing identity
	//   edit-account  — identity fields: name + write-only extra env
	//   edit-provider — one credential: aliases, soft limits, settings,
	//                   compatible-endpoint config, reauth, move
	type EditorMode = 'create' | 'add-provider' | 'edit-account' | 'edit-provider';
	let editor = $state<{ mode: EditorMode; accountId?: string; providerId?: string } | null>(null);

	const rows = $derived([...($accounts.data ?? [])]);
	const editingAccount = $derived(
		editor?.accountId ? rows.find((a) => a.id === editor?.accountId) : undefined
	);
	const editingProvider = $derived(
		editor?.providerId
			? editingAccount?.providers.find((p) => p.id === editor?.providerId)
			: undefined
	);

	let name = $state('');
	let provider = $state<ProviderKind>('anthropic');
	let refreshToken = $state('');
	// Compatible-endpoint fields (CCT-399): base URL, a static credential, the
	// auth scheme, and a tiny model-list editor (model code + display label).
	let baseUrl = $state('');
	let credential = $state('');
	// `keep` is an edit-only sentinel: leave the stored scheme untouched (it is
	// never read back, CCT-402). Create always picks bearer/api_key.
	let authScheme = $state<'bearer' | 'api_key' | 'keep'>('bearer');
	let modelRows = $state<{ model: string; label: string }[]>([{ model: '', label: '' }]);
	// Per-provider model alias map (CCT-406): logical name → concrete model code,
	// e.g. opus → claude-opus-4-8[1m]. Resolved server-side at spawn.
	let aliasRows = $state<{ alias: string; model: string }[]>([]);
	// Per-provider soft limits (CCT-411): cap cctui's own share of the 5h/7d usage
	// windows so it leaves headroom for the human sharing the subscription. Empty
	// input = no cap on that window.
	// `<Input type="number">` makes Svelte coerce `bind:value` to `number | null`
	// (null when the field is cleared), so these hold numbers, not strings.
	let soft5h = $state<number | null>(null);
	let soft7d = $state<number | null>(null);
	let softBypass = $state<number | null>(null);
	const isCompatible = $derived(provider.endsWith('-compatible'));

	// Per-provider settings (CCT-541) + per-account env (CCT-538). `settings`
	// mirrors the provider's settings_json; `envRows` feed the identity's
	// write-only env_json (never read back, so they start empty on edit);
	// `replaceEnv` gates whether env_json is sent at all.
	let acctSettings = $state<Record<string, unknown>>({});
	let acctEnvRows = $state<{ name: string; value: string }[]>([]);
	let acctReplaceEnv = $state(false);
	// Move-provider target (CCT-558 merge path): another account of the same owner.
	let moveTarget = $state('');

	/** Normalise a soft-limit input: empty ⇒ null, else a clamped non-negative
	 *  integer. Tolerates either the number a number-input binds or a stray string. */
	function softNum(v: number | string | null | undefined): number | null {
		if (v === null || v === undefined || v === '') return null;
		const n = Math.round(Number(v));
		return Number.isFinite(n) ? Math.max(0, n) : null;
	}

	/** The soft-limit block to send on save (always sent so clearing works). */
	function softLimits() {
		return {
			soft_limit_5h_pct: softNum(soft5h),
			soft_limit_7d_pct: softNum(soft7d),
			soft_limit_bypass_minutes: softNum(softBypass)
		};
	}

	/** Collapse the alias rows into the `{alias: model}` object the API expects,
	 *  dropping incomplete rows. */
	function aliasObject(): Record<string, string> {
		const out: Record<string, string> = {};
		for (const r of aliasRows) {
			const a = r.alias.trim();
			const m = r.model.trim();
			if (a && m) out[a] = m;
		}
		return out;
	}

	/** Collapse the env rows into the `{name: value}` object PATCH expects,
	 *  dropping rows without a name. An empty object clears the stored env. */
	function envObject(): Record<string, string> {
		const out: Record<string, string> = {};
		for (const r of acctEnvRows) {
			const n = r.name.trim();
			if (n) out[n] = r.value;
		}
		return out;
	}

	/** Trimmed model rows for the compatible-endpoint payloads. */
	function modelList() {
		return modelRows
			.map((r) => ({ model: r.model.trim(), label: r.label.trim() || r.model.trim() }))
			.filter((r) => r.model);
	}

	// "Sign in with Claude" OAuth flow state (CCT-243).
	let oauthNonce = $state<string | null>(null);
	let oauthCode = $state('');
	let oauthBusy = $state(false);
	let showAdvanced = $state(false);
	// Reauth mode (CCT-512): editing an existing native provider to refresh its
	// rejected credentials, which reveals the sign-in block inside the edit modal.
	let reauthing = $state(false);
	// OAuth attach target (CCT-558): when adding/reauthenticating, finish the flow
	// as a provider under this existing account instead of creating a new identity.
	let oauthAttachAccountId = $state<string | null>(null);

	function resetForm() {
		name = '';
		provider = 'anthropic';
		refreshToken = '';
		baseUrl = '';
		credential = '';
		authScheme = 'bearer';
		modelRows = [{ model: '', label: '' }];
		aliasRows = [];
		soft5h = null;
		soft7d = null;
		softBypass = null;
		oauthNonce = null;
		oauthCode = '';
		oauthBusy = false;
		showAdvanced = false;
		reauthing = false;
		oauthAttachAccountId = null;
		acctSettings = {};
		acctEnvRows = [];
		acctReplaceEnv = false;
		moveTarget = '';
	}

	// Start the authorize leg: ask the server for an authorize URL, open it in a
	// new tab, and reveal the paste field. Works for both Claude (anthropic) and
	// "Sign in with ChatGPT" for Codex (openai) — CCT-243/CCT-244.
	async function startOAuthLogin() {
		if (isAdmin && !ownerId && !oauthAttachAccountId) {
			toasts.err('Pick the owning user first');
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
			window.open(r.authorize_url, '_blank', 'noopener');
			if (provider === 'openai') {
				toasts.ok(
					'Opened ChatGPT — authorize, then copy the localhost:1455 URL and paste it below',
				);
			} else {
				toasts.ok('Opened claude.ai — authorize, then paste the code below');
			}
		} catch (e) {
			toasts.err((e as Error).message);
		} finally {
			oauthBusy = false;
		}
	}

	// Finish: exchange the pasted code/callback URL for tokens and create the
	// account/provider. Claude sends `code` (the code#state pair); Codex sends
	// `callback_url` (the full localhost:1455 URL from the address bar). With an
	// attach target (CCT-558) the credential lands under that account and the
	// name is ignored server-side.
	async function finishOAuthLogin() {
		if (!oauthAttachAccountId && !name.trim()) {
			toasts.err('Name is required');
			return;
		}
		if (!oauthNonce || !oauthCode.trim()) {
			toasts.err(
				provider === 'openai'
					? 'Paste the localhost:1455 URL first'
					: 'Paste the code from claude.ai first',
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
			toasts.ok(oauthAttachAccountId ? 'Provider added' : 'Account added');
			close();
		} catch (e) {
			toasts.err((e as Error).message);
		} finally {
			oauthBusy = false;
		}
	}

	function openCreate() {
		resetForm();
		editor = { mode: 'create' };
	}

	/** Provider kinds this account can still add: at most one per family
	 *  (anthropic/openai), mirroring the server's unique index (CCT-558). */
	function availableKinds(a: OAuthAccount): ProviderKind[] {
		const taken = new Set(a.providers.map((p) => p.family));
		return PROVIDER_KINDS.map((k) => k.value).filter((v) => !taken.has(providerFamily(v)));
	}

	function openAddProvider(a: OAuthAccount) {
		resetForm();
		editor = { mode: 'add-provider', accountId: a.id };
		provider = availableKinds(a)[0] ?? 'anthropic';
		ownerId = a.user_id;
		// The native OAuth flows attach via oauth/start's account_id (CCT-558).
		oauthAttachAccountId = a.id;
	}

	function openEditAccount(a: OAuthAccount) {
		resetForm();
		editor = { mode: 'edit-account', accountId: a.id };
		name = a.name;
		// env_json is write-only: rows start empty; editing them flips replaceEnv.
	}

	function openEditProvider(a: OAuthAccount, p: AccountProvider) {
		resetForm();
		editor = { mode: 'edit-provider', accountId: a.id, providerId: p.id };
		provider = p.provider as ProviderKind;
		// Compatible endpoints can edit their model list in place (CCT-402). The
		// base URL, credential, and scheme are never read back, so they start
		// blank/"keep" — supplying one overwrites, leaving it keeps the stored value.
		if (p.provider.endsWith('-compatible')) {
			const ms = p.models ?? [];
			modelRows = ms.length
				? ms.map((m) => ({ model: m.model, label: m.label }))
				: [{ model: '', label: '' }];
			authScheme = 'keep';
		}
		// Aliases are editable for every provider (CCT-406).
		aliasRows = Object.entries(p.model_aliases ?? {}).map(([alias, model]) => ({ alias, model }));
		// Soft limits are editable for every provider (CCT-411).
		soft5h = p.soft_limit_5h_pct;
		soft7d = p.soft_limit_7d_pct;
		softBypass = p.soft_limit_bypass_minutes;
		// Settings are editable per provider (CCT-541/CCT-560).
		acctSettings = { ...(p.settings_json ?? {}) };
	}

	// Reauthenticate a flagged provider (CCT-512): open its edit modal, flip into
	// reauth mode (reveals the sign-in block), and kick the authorize leg. The
	// pasted code is exchanged by finishOAuthLogin, which refreshes the
	// same-family credential in place and clears `needs_reauth` server-side.
	function reauth(a: OAuthAccount, p: AccountProvider) {
		openEditProvider(a, p);
		ownerId = a.user_id;
		reauthing = true;
		// Attach the refreshed credential to THIS account (CCT-558) rather than
		// creating a new identity from the name.
		oauthAttachAccountId = a.id;
		startOAuthLogin();
	}

	function close() {
		editor = null;
	}

	async function save() {
		const mode = editor?.mode;
		const model_aliases = aliasObject();
		try {
			if (mode === 'edit-account' && editor?.accountId) {
				if (!name.trim()) {
					toasts.err('Name is required');
					return;
				}
				const identity: UpdateAccount = { name: name.trim() };
				if (acctReplaceEnv) identity.env_json = envObject();
				await actions.update(editor.accountId, identity);
				toasts.ok('Account updated');
			} else if (mode === 'edit-provider' && editor?.accountId && editor.providerId) {
				// Always send the alias map + soft limits + settings so clearing
				// them sticks (empty object clears the stored blob). Settings only
				// apply to the claude-code harness → anthropic-family providers.
				const body: UpdateProvider = {
					model_aliases,
					soft_limits: softLimits(),
					...(editingProvider?.family === 'anthropic' ? { settings_json: acctSettings } : {})
				};
				if (isCompatible) {
					body.models = modelList();
					if (baseUrl.trim()) body.base_url = baseUrl.trim();
					if (credential.trim()) body.access_token = credential.trim();
					if (authScheme !== 'keep') body.auth_scheme = authScheme;
				}
				await actions.updateProvider(editor.accountId, editor.providerId, body);
				toasts.ok('Provider updated');
			} else if (mode === 'add-provider' && editor?.accountId) {
				// Native OAuth adds go through finishOAuthLogin instead; this path is
				// the compatible-endpoint / pasted-refresh-token attach (CCT-558).
				const spec: CreateProvider = {
					provider,
					...(Object.keys(model_aliases).length ? { model_aliases } : {}),
					...softLimits()
				};
				if (isCompatible) {
					if (!baseUrl.trim()) {
						toasts.err('Base URL is required for a compatible endpoint');
						return;
					}
					spec.base_url = baseUrl.trim();
					spec.auth_scheme = authScheme === 'keep' ? 'bearer' : authScheme;
					if (credential.trim()) spec.access_token = credential.trim();
					const models = modelList();
					if (models.length) spec.models = models;
				} else {
					if (!refreshToken.trim()) {
						toasts.err('Refresh token is required');
						return;
					}
					spec.refresh_token = refreshToken.trim();
				}
				await actions.addProvider(editor.accountId, spec);
				toasts.ok('Provider added');
			} else {
				// create: identity + first credential in one call.
				if (!name.trim()) {
					toasts.err('Name is required');
					return;
				}
				if (isAdmin && !ownerId) {
					toasts.err('Pick the owning user first');
					return;
				}
				let body: CreateAccount;
				if (isCompatible) {
					if (!baseUrl.trim()) {
						toasts.err('Base URL is required for a compatible endpoint');
						return;
					}
					const models = modelList();
					body = {
						name: name.trim(),
						provider,
						base_url: baseUrl.trim(),
						auth_scheme: authScheme === 'keep' ? 'bearer' : authScheme,
						...(credential.trim() ? { access_token: credential.trim() } : {}),
						...(models.length ? { models } : {}),
						...(Object.keys(model_aliases).length ? { model_aliases } : {}),
						...softLimits(),
						...(isAdmin ? { user_id: ownerId } : {}),
					};
				} else {
					if (!refreshToken.trim()) {
						toasts.err('Refresh token is required');
						return;
					}
					body = {
						name: name.trim(),
						provider,
						refresh_token: refreshToken.trim(),
						...(Object.keys(model_aliases).length ? { model_aliases } : {}),
						...softLimits(),
						...(isAdmin ? { user_id: ownerId } : {}),
					};
				}
				await actions.create(body);
				toasts.ok('Account added');
			}
			close();
		} catch (e) {
			toasts.err((e as Error).message);
		}
	}

	function removeAccount(a: OAuthAccount) {
		if (!confirm(`Delete account "${a.name}" and all its provider credentials?`)) return;
		guard(actions.remove(a.id).then(() => toasts.ok('Deleted')));
	}

	function removeProvider(a: OAuthAccount, p: AccountProvider) {
		if (
			!confirm(
				`Remove the ${providerLabel(p.provider)} credential from "${a.name}"? The account and its other providers stay.`
			)
		)
			return;
		guard(actions.removeProvider(a.id, p.id).then(() => toasts.ok('Provider removed')));
	}

	async function moveProvider() {
		if (!editor?.accountId || !editor.providerId || !moveTarget) return;
		try {
			await actions.moveProvider(editor.accountId, editor.providerId, moveTarget);
			toasts.ok('Provider moved');
			close();
		} catch (e) {
			toasts.err((e as Error).message);
		}
	}

	/** Same-owner move targets whose family slot is free (server 409s otherwise). */
	const moveTargets = $derived(
		editingAccount && editingProvider
			? rows.filter(
					(a) =>
						a.id !== editingAccount.id &&
						a.user_id === editingAccount.user_id &&
						!a.providers.some((p) => p.family === editingProvider.family)
				)
			: []
	);

	// An account whose every provider is server-managed (the litellm shim) is
	// read-only as a whole; per-provider buttons key off each row's `managed`.
	const isManaged = (a: OAuthAccount) => a.providers.length > 0 && a.providers.every((p) => p.managed);

	// Native OAuth flows save via finishOAuthLogin (the pasted-code exchange).
	const oauthSaves = $derived(
		editor !== null &&
			(editor?.mode === 'create' || editor?.mode === 'add-provider' || reauthing) &&
			!isCompatible &&
			oauthNonce !== null &&
			!showAdvanced
	);

	const modalTitle = $derived(
		editor?.mode === 'create'
			? 'New account'
			: editor?.mode === 'add-provider'
				? `Add provider to "${editingAccount?.name ?? ''}"`
				: editor?.mode === 'edit-account'
					? 'Edit account'
					: reauthing
						? 'Reauthenticate provider'
						: `Edit ${providerLabel(editingProvider?.provider ?? '')} provider`
	);
</script>

<Heading level={1} class="page-title">Accounts</Heading>

<Tabs {tabs} bind:value={tab} label="Account sections">
	{#snippet panel(id)}
		{#if id === 'ai'}
			<Cluster class="bar" justify="space-between" align="center" gap="var(--sp-3)">
				<Text as="p" tone="muted" size="sm" class="intro">
					Named accounts for Claude and Codex, plus self-hosted OpenAI/Anthropic-compatible
					endpoints. An account can hold one credential per provider family; pick one per job
					at spawn time and the session runs through a passthrough gateway under it. Tokens
					are stored encrypted and never shown again.
				</Text>
				<Button control variant="primary" onclick={openCreate}>+ New account</Button>
			</Cluster>

			{#if $accounts.isLoading}
				<div class="empty"><span class="spin"></span></div>
			{:else if rows.length === 0}
				<div class="empty"><Text tone="muted">No accounts yet.</Text></div>
			{:else}
				<AutoGrid min="22rem" gap="var(--sp-3)">
					{#each rows as a (a.id)}
						<Card class="account-card">
							<Stack gap="var(--sp-3)" class="card-body">
								<Heading level={2} size="lg" class="account-name">{a.name}</Heading>

								<!-- One panel per provider credential (CCT-560). -->
								{#each a.providers as p (p.id)}
									<ProviderPanel
										provider={p}
										usageEnabled={tab === 'ai'}
										canManage={!p.managed}
										canRemove={!p.managed}
										onedit={() => openEditProvider(a, p)}
										onreauth={() => reauth(a, p)}
										onremove={() => removeProvider(a, p)}
									/>
								{:else}
									<Text tone="faint" size="sm">No provider credentials yet.</Text>
								{/each}
								{#if !isManaged(a) && availableKinds(a).length}
									<Button size="sm" style="align-self: flex-start" onclick={() => openAddProvider(a)}>
										+ Add provider
									</Button>
								{/if}

								<dl class="stats">
									{#if isAdmin}
										<div><dt>Owner</dt><dd>{a.user_name ?? '—'}</dd></div>
									{/if}
									<div><dt>Created</dt><dd><Timestamp value={a.created_at} mode="date" tone="inherit" /></dd></div>
								</dl>
								{#if !isManaged(a) && (isAdmin || a.user_id === $me.data?.user_id)}
									<!-- Sharing management (CCT-510): owner-only surface to view/grant/
									     revoke who may USE this account. The list endpoint is
									     owner-scoped, so only render (and fetch) it for the owner/admin. -->
									<AccountShares id={a.id} enabled={tab === 'ai'} />
								{/if}
							</Stack>
							<Cluster as="footer" gap="var(--sp-1)" justify="flex-end" class="card-foot">
								{#if isManaged(a)}
									<Text tone="faint" size="xs">Managed (read-only)</Text>
								{:else}
									<Button onclick={() => openEditAccount(a)}>Edit</Button>
									<Button variant="danger" onclick={() => removeAccount(a)}>Delete</Button>
								{/if}
							</Cluster>
						</Card>
					{/each}
				</AutoGrid>
			{/if}
		{:else if id === 'connectors'}
			<GithubConnectors />
		{:else if id === 'dispatchers'}
			<DispatchersPanel heading={false} />
		{/if}
	{/snippet}
</Tabs>

{#if editor !== null}
	<Modal title={modalTitle} onclose={close}>
		{#snippet body()}
			<div class="editor-body">
				{#if editor?.mode === 'create' || editor?.mode === 'edit-account'}
					<Field label="Name">
						<Input bind:value={name} placeholder="personal" />
					</Field>
				{/if}
				{#if editor?.mode === 'create' && isAdmin}
					<Field label="Owner">
						<Select bind:value={ownerId}>
							{#each activeUsers as u (u.id)}
								<option value={u.id}>{u.name}</option>
							{/each}
						</Select>
					</Field>
				{/if}
				{#if editor?.mode === 'create' || editor?.mode === 'add-provider'}
					<Field label="Provider">
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
					<!-- Identity half (CCT-560): the write-only extra env lives on the
					     account; provider settings are edited per provider. -->
					<AccountSettingsEditor
						bind:envRows={acctEnvRows}
						bind:replaceEnv={acctReplaceEnv}
						showSettings={false}
					/>
				{:else}
					{#if isCompatible}
						<!-- Compatible endpoint (CCT-399): base URL + a static credential +
						     a model list. No OAuth; the gateway forwards the credential and
						     skips refresh. On edit (CCT-402) the model list is editable in
						     place; base URL / credential / scheme are write-only — blank or
						     "keep" leaves the stored value untouched. -->
						{@const isEdit = editor?.mode === 'edit-provider'}
						<Field label="Base URL">
							<Input
								bind:value={baseUrl}
								placeholder={isEdit ? 'leave blank to keep current' : 'https://litellm.example/v1'}
							/>
						</Field>
						<Field label="Auth scheme">
							<Select bind:value={authScheme}>
								{#if isEdit}
									<option value="keep">Keep current</option>
								{/if}
								<option value="bearer">Bearer token</option>
								<option value="api_key">API key</option>
							</Select>
						</Field>
						<Field label="Credential (optional)">
							<Input
								type="password"
								bind:value={credential}
								placeholder={isEdit
									? 'leave blank to keep current'
									: 'bearer / API key (blank for an open proxy)'}
							/>
						</Field>
						<div class="models">
							<Text as="div" tone="muted" size="sm">Models</Text>
							{#each modelRows as row, i (i)}
								<div class="model-row">
									<Input bind:value={row.model} placeholder="model code (e.g. qwen3-coder)" />
									<Input bind:value={row.label} placeholder="label (optional)" />
									<Button
										variant="danger"
										onclick={() => (modelRows = modelRows.filter((_, j) => j !== i))}
										disabled={modelRows.length === 1}>✕</Button
									>
								</div>
							{/each}
							<Button
								onclick={() => (modelRows = [...modelRows, { model: '', label: '' }])}
								>+ Add model</Button
							>
						</div>
					{:else if editor?.mode === 'create' || editor?.mode === 'add-provider' || reauthing}
						<!-- Sign in with Claude / ChatGPT: authorize upstream, paste back.
						     Also shown when reauthenticating an existing provider (CCT-512). -->
						{#if !oauthNonce}
							<Button
								variant="primary"
								style="align-self: flex-start"
								disabled={oauthBusy}
								onclick={startOAuthLogin}
							>
								{oauthBusy
									? 'Opening…'
									: provider === 'openai'
										? 'Sign in with ChatGPT'
										: 'Sign in with Claude'}
							</Button>
						{:else}
							<Field label={provider === 'openai' ? 'URL from ChatGPT' : 'Code from claude.ai'}>
								<Input
									bind:value={oauthCode}
									placeholder={provider === 'openai'
										? 'paste the http://localhost:1455/auth/callback?... URL'
										: 'paste the code#state shown after authorizing'}
								/>
							</Field>
							{#if provider === 'openai'}
								<Text as="p" tone="muted" size="sm">
									The browser tab will fail to load localhost:1455 — that's expected.
									Copy the full URL from its address bar and paste it above.
								</Text>
							{/if}
							<Text as="p" tone="muted" size="sm">
								Didn't get {provider === 'openai' ? 'a URL' : 'a code'}?
								<Link onclick={startOAuthLogin}
									>Open {provider === 'openai' ? 'ChatGPT' : 'claude.ai'} again</Link
								>
							</Text>
						{/if}
						{#if !reauthing}
							<details bind:open={showAdvanced} class="adv">
								<summary><Text tone="muted" size="sm">Advanced: paste a refresh token instead</Text></summary>
								<Field label="OAuth refresh token" class="adv-fld">
									<Input
										type="password"
										bind:value={refreshToken}
										placeholder="paste the OAuth refresh token"
									/>
								</Field>
							</details>
						{/if}
					{/if}

					<!-- Model aliases (CCT-406): logical name -> concrete model code,
					     resolved server-side at spawn; works for every provider. -->
					{#if editor?.mode === 'edit-provider' || isCompatible}
						<div class="models">
							<Text as="div" tone="muted" size="sm">Model aliases</Text>
							<Text as="div" tone="faint" size="xs">
								Map a logical name to the model this account launches (e.g.
								<code>opus</code> -> <code>claude-opus-4-8[1m]</code>).
							</Text>
							{#each aliasRows as row, i (i)}
								<div class="model-row">
									<Input bind:value={row.alias} placeholder="logical name (e.g. opus)" />
									<Input bind:value={row.model} placeholder="model code (e.g. claude-opus-4-8[1m])" />
									<Button
										variant="danger"
										onclick={() => (aliasRows = aliasRows.filter((_, j) => j !== i))}>✕</Button
									>
								</div>
							{/each}
							<Button onclick={() => (aliasRows = [...aliasRows, { alias: '', model: '' }])}
								>+ Add alias</Button
							>
						</div>
					{/if}

					<!-- Soft limits (CCT-411): cap cctui's own share of the 5h/7d usage
					     windows so a shared subscription keeps headroom for the human.
					     Blank ⇒ no cap on that window. Works for anthropic (upstream usage
					     API) and openai (locally metered, CCT-511). -->
					{#if provider === 'anthropic' || provider === 'anthropic-compatible' || provider === 'openai'}
						<div class="models">
							<Text as="div" tone="muted" size="sm">Soft limits</Text>
							<Text as="div" tone="faint" size="xs">
								Cap cctui's own share of each usage window (%). Over the cap, cctui's
								spawned workers get a 429 instead of consuming more — leaving headroom
								for your own Claude Code. Blank = no cap. Bypass ignores a cap when the
								window resets within that many minutes.
							</Text>
							<div class="soft-grid">
								<label class="soft-field">
									<Text as="div" tone="faint" size="xs">5h cap %</Text>
									<Input type="number" bind:value={soft5h} placeholder="e.g. 80" />
								</label>
								<label class="soft-field">
									<Text as="div" tone="faint" size="xs">7d cap %</Text>
									<Input type="number" bind:value={soft7d} placeholder="e.g. 80" />
								</label>
								<label class="soft-field">
									<Text as="div" tone="faint" size="xs">Bypass (min)</Text>
									<Input type="number" bind:value={softBypass} placeholder="e.g. 30" />
								</label>
							</div>
						</div>
					{/if}

					<!-- Per-provider settings (CCT-541/CCT-560). Only the claude-code
					     harness has an injectable settings.json today, so only
					     anthropic-family providers get the toggle list. -->
					{#if editor?.mode === 'edit-provider' && editingProvider}
						{#if editingProvider.family === 'anthropic'}
							<AccountSettingsEditor bind:settings={acctSettings} showEnv={false} />
						{:else}
							<Text tone="faint" size="sm">
								No per-provider settings for Codex yet — model aliases and soft
								limits above are the available knobs.
							</Text>
						{/if}

						<!-- Move (CCT-558): re-parent this credential onto another account of
						     the same owner — the merge path for migrated split rows. -->
						{#if !reauthing && moveTargets.length}
							<div class="models">
								<Text as="div" tone="muted" size="sm">Move to another account</Text>
								<Text as="div" tone="faint" size="xs">
									Re-parent this credential onto another of this owner's accounts
									(e.g. merging "alice (anthropic)" + "alice (openai)" into one
									"alice"). Only accounts with a free {editingProvider.family} slot
									are listed.
								</Text>
								<div class="move-row">
									<Select bind:value={moveTarget}>
										<option value="">Pick an account…</option>
										{#each moveTargets as t (t.id)}
											<option value={t.id}>{t.name}</option>
										{/each}
									</Select>
									<Button disabled={!moveTarget} onclick={moveProvider}>Move</Button>
								</div>
							</div>
						{/if}
					{/if}
				{/if}
			</div>
		{/snippet}
		{#snippet footer()}
			<div class="spacer"></div>
			<Button onclick={close}>Cancel</Button>
			{#if oauthSaves}
				<Button variant="primary" disabled={oauthBusy} onclick={finishOAuthLogin}>Save</Button>
			{:else}
				<Button variant="primary" onclick={save}>Save</Button>
			{/if}
		{/snippet}
	</Modal>
{/if}

<style>
	:global(.page-title) {
		margin-bottom: var(--sp-3);
	}
	:global(.bar) {
		margin-bottom: var(--sp-3);
	}
	/* Intro copy shares the header row with the New-account button; it's passed
	   to a Text atom (renders inside it), so cap its width globally. */
	:global(.bar .intro) {
		max-width: 60ch;
	}
	/* Cards stretch to the tallest in their row (AutoGrid), then the body grows
	   so the footer's action buttons pin to the bottom edge — consistent across
	   cards regardless of how many provider panels are present. */
	:global(.account-card) {
		display: flex;
		flex-direction: column;
		height: 100%;
	}
	:global(.account-card .card-body) {
		flex: 1 1 auto;
		min-width: 0;
	}
	:global(.account-card .account-name) {
		min-width: 0;
		word-break: break-word;
	}
	:global(.account-card .card-foot) {
		margin-top: var(--sp-3);
		padding-top: var(--sp-3);
		border-top: 1px solid var(--border);
	}
	.soft-grid {
		display: grid;
		grid-template-columns: repeat(3, 1fr);
		gap: var(--sp-2);
	}
	.soft-field {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
		min-width: 0;
	}
	/* Account-level stat list — label over value, no input-like chrome (CCT-345). */
	.stats {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(7rem, 1fr));
		gap: var(--sp-2) var(--sp-3);
		margin: 0;
	}
	.stats div {
		min-width: 0;
	}
	.stats dt {
		color: var(--text-muted);
		font-size: var(--fs-xs);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}
	.stats dd {
		margin: 0.1rem 0 0;
		font-size: var(--fs-sm);
		font-weight: var(--fw-medium);
		overflow-wrap: anywhere;
	}
	.editor-body {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
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
	.move-row {
		display: grid;
		grid-template-columns: 1fr auto;
		gap: var(--sp-2);
		align-items: center;
	}
	.adv :global(.adv-fld) {
		margin-top: var(--sp-2);
	}
</style>
