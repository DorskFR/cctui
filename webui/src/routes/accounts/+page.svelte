<script lang="ts">
	import {
		useAccounts,
		useAccountActions,
		useCapabilities,
		useMe,
		useUsers,
		primaryProvider,
		type OAuthAccount,
		type CreateAccount,
		type UpdateAccount,
		type UpdateProvider,
	} from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { compact } from '$lib/format';
	import UsageBars from '$lib/components/molecules/UsageBars.svelte';
	import AccountShares from '$lib/components/molecules/AccountShares.svelte';
	import AdapterIcon from '$lib/components/atoms/AdapterIcon.svelte';
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
	import { providerLabel } from './accounts.logic';

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

	// Editor state. `editing` holds the id when renaming, null when creating a
	// fresh one, undefined when the editor is closed.
	let editing = $state<string | null | undefined>(undefined);
	let name = $state('');
	let provider = $state<
		'anthropic' | 'openai' | 'anthropic-compatible' | 'openai-compatible'
	>('anthropic');
	let refreshToken = $state('');
	// Compatible-endpoint fields (CCT-399): base URL, a static credential, the
	// auth scheme, and a tiny model-list editor (model code + display label).
	let baseUrl = $state('');
	let credential = $state('');
	// `keep` is an edit-only sentinel: leave the stored scheme untouched (it is
	// never read back, CCT-402). Create always picks bearer/api_key.
	let authScheme = $state<'bearer' | 'api_key' | 'keep'>('bearer');
	let modelRows = $state<{ model: string; label: string }[]>([{ model: '', label: '' }]);
	// Per-account model alias map (CCT-406): logical name → concrete model code,
	// e.g. opus → claude-opus-4-8[1m]. Applies to every provider; resolved
	// server-side at spawn. Edited as rows, sent as an object.
	let aliasRows = $state<{ alias: string; model: string }[]>([]);
	// Per-account soft limits (CCT-411): cap cctui's own share of the 5h/7d usage
	// windows so it leaves headroom for the human sharing the subscription. Empty
	// input = no cap on that window. Kept as strings so blank ⇒ null.
	// `<Input type="number">` makes Svelte coerce `bind:value` to `number | null`
	// (null when the field is cleared), so these hold numbers, not strings.
	let soft5h = $state<number | null>(null);
	let soft7d = $state<number | null>(null);
	let softBypass = $state<number | null>(null);
	const isCompatible = $derived(provider.endsWith('-compatible'));

	// Per-account settings editor state (CCT-541). `settings` mirrors the provider's
	// settings_json (SAFE/CARE keys); `envRows` feed the write-only env_json (never
	// read back, so they start empty on edit); `replaceEnv` gates whether env_json
	// is sent at all (only when the operator actually edits the env). The launch
	// defaults surface was removed with CCT-558 (superseded by CCT-561).
	let acctSettings = $state<Record<string, unknown>>({});
	let acctEnvRows = $state<{ name: string; value: string }[]>([]);
	let acctReplaceEnv = $state(false);

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

	// "Sign in with Claude" OAuth flow state (CCT-243).
	let oauthNonce = $state<string | null>(null);
	let oauthCode = $state('');
	let oauthBusy = $state(false);
	let showAdvanced = $state(false);
	// Reauth mode (CCT-512): editing an existing native account to refresh its
	// rejected credentials, which reveals the sign-in block inside the edit modal.
	let reauthing = $state(false);
	// OAuth attach target (CCT-558): when reauthenticating, finish the flow as a
	// provider under this existing account instead of creating a new identity.
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
	}

	// Start the authorize leg: ask the server for an authorize URL, open it in a
	// new tab, and reveal the paste field. Works for both Claude (anthropic) and
	// "Sign in with ChatGPT" for Codex (openai) — CCT-243/CCT-244.
	async function startOAuthLogin() {
		if (isAdmin && !ownerId) {
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
	// account. Claude sends `code` (the code#state pair); Codex sends
	// `callback_url` (the full localhost:1455 URL from the address bar).
	async function finishOAuthLogin() {
		if (!name.trim()) {
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
			await actions.oauthFinish(
				provider === 'openai'
					? { nonce: oauthNonce, name: name.trim(), callback_url: oauthCode.trim() }
					: { nonce: oauthNonce, name: name.trim(), code: oauthCode.trim() },
			);
			toasts.ok('Account added');
			close();
		} catch (e) {
			toasts.err((e as Error).message);
		} finally {
			oauthBusy = false;
		}
	}

	function openCreate() {
		resetForm();
		editing = null;
	}

	function openEdit(a: OAuthAccount) {
		resetForm();
		editing = a.id;
		name = a.name;
		// TODO(CCT-560): the editor still assumes a single credential per account —
		// edit the first provider row until the accounts UI is redesigned for
		// multi-provider identities.
		const p = primaryProvider(a);
		provider = (p?.provider as typeof provider) ?? 'anthropic';
		// Compatible endpoints can edit their model list in place (CCT-402). The
		// base URL, credential, and scheme are never read back, so they start
		// blank/"keep" — supplying one overwrites, leaving it keeps the stored value.
		if (provider.endsWith('-compatible')) {
			const ms = p?.models ?? [];
			modelRows = ms.length
				? ms.map((m) => ({ model: m.model, label: m.label }))
				: [{ model: '', label: '' }];
			authScheme = 'keep';
		}
		// Aliases are editable for every provider (CCT-406).
		aliasRows = Object.entries(p?.model_aliases ?? {}).map(([alias, model]) => ({ alias, model }));
		// Soft limits are editable for every provider (CCT-411).
		soft5h = p?.soft_limit_5h_pct ?? null;
		soft7d = p?.soft_limit_7d_pct ?? null;
		softBypass = p?.soft_limit_bypass_minutes ?? null;
		// Settings are editable for every provider (CCT-541). settings_json comes
		// back from the API; env_json is write-only and never returned, so env rows
		// start empty (the operator re-enters to replace).
		acctSettings = { ...(p?.settings_json ?? {}) };
		acctEnvRows = [];
		acctReplaceEnv = false;
	}

	// Reauthenticate a flagged account (CCT-512): open its edit modal, flip into
	// reauth mode (reveals the sign-in block), and kick the authorize leg. The
	// pasted code is exchanged by finishOAuthLogin, which upserts the credentials
	// in place (same name+provider) and clears `needs_reauth` server-side.
	function reauth(a: OAuthAccount) {
		openEdit(a);
		ownerId = a.user_id;
		reauthing = true;
		// Attach the refreshed credential to THIS account (CCT-558) rather than
		// creating a new identity from the name.
		oauthAttachAccountId = a.id;
		startOAuthLogin();
	}

	function close() {
		editing = undefined;
	}

	async function save() {
		if (!name.trim()) {
			toasts.err('Name is required');
			return;
		}
		const model_aliases = aliasObject();
		try {
			if (editing) {
				// CCT-558: the edit is two PATCHes — identity fields (name, env_json)
				// go to the account; credential fields (aliases, soft limits, settings,
				// compatible-endpoint config) go to its provider row.
				const identity: UpdateAccount = { name: name.trim() };
				if (acctReplaceEnv) identity.env_json = envObject();
				await actions.update(editing, identity);
				// TODO(CCT-560): still single-credential — patch the first provider row.
				const providerId = editingAccount ? primaryProvider(editingAccount)?.id : undefined;
				if (providerId) {
					// Always send the alias map + soft limits + settings so clearing
					// them sticks (empty object clears the stored blob).
					const body: UpdateProvider = {
						model_aliases,
						soft_limits: softLimits(),
						settings_json: acctSettings
					};
					if (isCompatible) {
						const models = modelRows
							.map((r) => ({ model: r.model.trim(), label: r.label.trim() || r.model.trim() }))
							.filter((r) => r.model);
						body.models = models;
						if (baseUrl.trim()) body.base_url = baseUrl.trim();
						if (credential.trim()) body.access_token = credential.trim();
						if (authScheme !== 'keep') body.auth_scheme = authScheme;
					}
					await actions.updateProvider(editing, providerId, body);
				}
				toasts.ok('Account updated');
			} else {
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
					const models = modelRows
						.map((r) => ({ model: r.model.trim(), label: r.label.trim() || r.model.trim() }))
						.filter((r) => r.model);
					body = {
						name: name.trim(),
						provider,
						base_url: baseUrl.trim(),
						auth_scheme: authScheme,
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

	function remove(a: OAuthAccount) {
		if (!confirm(`Delete account "${a.name}"?`)) return;
		guard(actions.remove(a.id).then(() => toasts.ok('Deleted')));
	}

	const rows = $derived([...($accounts.data ?? [])]);
	// The account currently open in the edit modal (CCT-541) — drives the settings
	// editor's provider-specific model list.
	const editingAccount = $derived(editing ? rows.find((a) => a.id === editing) : undefined);
</script>

<Heading level={1} class="page-title">Accounts</Heading>

<Tabs {tabs} bind:value={tab} label="Account sections">
	{#snippet panel(id)}
		{#if id === 'ai'}
			<Cluster class="bar" justify="space-between" align="center" gap="var(--sp-3)">
				<Text as="p" tone="muted" size="sm" class="intro">
					Named accounts for Claude and Codex, plus self-hosted OpenAI/Anthropic-compatible
					endpoints. Pick one per job at spawn time; the session runs through a passthrough
					gateway under that account. Tokens are stored encrypted and never shown again.
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
			<!-- TODO(CCT-560): the card still renders a single credential — the first
			     provider row — until the accounts UI is redesigned for
			     multi-provider identities. -->
			{@const p = primaryProvider(a)}
			<Card class="account-card">
				<Stack gap="var(--sp-3)" class="card-body">
					<Cluster gap="var(--sp-2)" align="center" wrap={false}>
						<span class="provider-mark" title={providerLabel(p?.provider ?? '')}>
							<AdapterIcon provider={p?.provider ?? ''} size={22} />
						</span>
						<Heading level={2} size="lg" class="account-name">{a.name}</Heading>
					</Cluster>
					{#if p?.needs_reauth}
						<!-- Credential rejected (CCT-512): the gateway saw the upstream
						     provider reject this account's OAuth grant. -->
						<div class="reauth-banner" title={p.last_auth_error ?? undefined}>
							<Text as="span" size="xs">⚠ Credential rejected — reauthenticate</Text>
						</div>
					{/if}
					{#if p && (p.provider === 'anthropic' || p.provider === 'openai')}
						<div class="usage-block">
							<Text as="div" tone="muted" size="xs" class="usage-head">Subscription usage</Text>
							<UsageBars
								id={p.id}
								provider={p.provider}
								cap5h={p.soft_limit_5h_pct}
								cap7d={p.soft_limit_7d_pct}
							/>
						</div>
					{/if}
					<dl class="stats">
						{#if isAdmin}
							<div><dt>Owner</dt><dd>{a.user_name ?? '—'}</dd></div>
						{/if}
						<div><dt>Requests</dt><dd>{compact(p?.request_count ?? 0)}</dd></div>
						<div><dt>Last used</dt><dd><Timestamp value={p?.last_used_at ?? null} mode="relative" tone="inherit" /></dd></div>
						<div><dt>Created</dt><dd><Timestamp value={a.created_at} mode="date" tone="inherit" /></dd></div>
					</dl>
					{#if !p?.managed && (isAdmin || a.user_id === $me.data?.user_id)}
						<!-- Sharing management (CCT-510): owner-only surface to view/grant/
						     revoke who may USE this account. The list endpoint is
						     owner-scoped, so only render (and fetch) it for the owner/admin. -->
						<AccountShares id={a.id} enabled={tab === 'ai'} />
					{/if}
				</Stack>
				<Cluster as="footer" gap="var(--sp-1)" justify="flex-end" class="card-foot">
					{#if p?.managed}
						<Text tone="faint" size="xs">Managed (read-only)</Text>
					{:else}
						{#if p?.needs_reauth && !p.provider.endsWith('-compatible')}
							<Button variant="primary" onclick={() => reauth(a)}>Reauthenticate</Button>
						{/if}
						<Button onclick={() => openEdit(a)}>Edit</Button>
						<Button variant="danger" onclick={() => remove(a)}>Delete</Button>
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

{#if editing !== undefined}
	<Modal title={reauthing ? 'Reauthenticate account' : editing ? 'Edit account' : 'New account'} onclose={close}>
		{#snippet body()}
			<div class="editor-body">
				<Field label="Name">
					<Input bind:value={name} placeholder="personal" />
				</Field>
				{#if !editing && isAdmin}
					<Field label="Owner">
						<Select bind:value={ownerId}>
							{#each activeUsers as u (u.id)}
								<option value={u.id}>{u.name}</option>
							{/each}
						</Select>
					</Field>
				{/if}
				{#if !editing}
					<Field label="Provider">
						<Select
							bind:value={provider}
							onchange={() => {
								oauthNonce = null;
								oauthCode = '';
							}}
						>
							<option value="anthropic">Claude (anthropic)</option>
							<option value="openai">Codex (openai)</option>
							<option value="anthropic-compatible">Anthropic-compatible endpoint</option>
							<option value="openai-compatible">OpenAI-compatible endpoint</option>
						</Select>
					</Field>
				{/if}

				{#if isCompatible}
						<!-- Compatible endpoint (CCT-399): base URL + a static credential +
						     a model list. No OAuth; the gateway forwards the credential and
						     skips refresh. On edit (CCT-402) the model list is editable in
						     place; base URL / credential / scheme are write-only — blank or
						     "keep" leaves the stored value untouched. -->
						<Field label="Base URL">
							<Input
								bind:value={baseUrl}
								placeholder={editing ? 'leave blank to keep current' : 'https://litellm.example/v1'}
							/>
						</Field>
						<Field label="Auth scheme">
							<Select bind:value={authScheme}>
								{#if editing}
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
								placeholder={editing
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
					{:else if !editing || reauthing}
						<!-- Sign in with Claude / ChatGPT: authorize upstream, paste back.
						     Also shown when reauthenticating an existing account (CCT-512). -->
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

					<!-- Model aliases (CCT-406): logical name -> concrete model code,
					     resolved server-side at spawn; works for every provider. -->
					{#if editing || isCompatible}
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

					<!-- Per-account settings + env (CCT-541). Edit only (persisted via
					     PATCH); the create flow signs in first. -->
					{#if editing && editingAccount}
						<AccountSettingsEditor
							bind:settings={acctSettings}
							bind:envRows={acctEnvRows}
							bind:replaceEnv={acctReplaceEnv}
						/>
					{/if}
			</div>
		{/snippet}
		{#snippet footer()}
			<div class="spacer"></div>
			<Button onclick={close}>Cancel</Button>
			{#if (!editing || reauthing) && oauthNonce && !showAdvanced}
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
	/* Provider brand mark — keeps the AdapterIcon's own tint (amber/blue) but
	   gives it a soft tile so it reads as an avatar, not inline text. */
	.provider-mark {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		flex: none;
		width: 2.25rem;
		height: 2.25rem;
		border-radius: var(--r-sm);
		background: var(--bg-elevated-2);
		border: 1px solid var(--border);
	}
	/* Cards stretch to the tallest in their row (AutoGrid), then the body grows
	   so the footer's action buttons pin to the bottom edge — consistent across
	   cards regardless of whether a usage block is present. */
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
	.usage-block {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-3);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		background: var(--bg-elevated-2);
	}
	/* Credential-rejected banner (CCT-512): a muted danger strip on the card. */
	.reauth-banner {
		padding: var(--sp-1) var(--sp-2);
		border: 1px solid var(--danger, #d9534f);
		border-radius: var(--r-sm);
		background: color-mix(in srgb, var(--danger, #d9534f) 12%, transparent);
		color: var(--danger, #d9534f);
	}
	/* Passed to a Text atom (renders inside it), so target globally. Size/colour
	   come from Text; the page owns only the uppercase treatment. */
	.usage-block :global(.usage-head) {
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}
	/* Lightweight stat list — label over value, no input-like chrome (CCT-345). */
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
	.adv :global(.adv-fld) {
		margin-top: var(--sp-2);
	}
</style>
