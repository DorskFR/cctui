<script lang="ts">
	import { errMessage } from '$lib/api';
	import {
		useAccounts,
		useAccountActions,
		useAccountUsage,
		useMe,
		useUsers,
		type OAuthAccount,
		type AccountModel,
		type AccountProvider,
		type CreateAccount,
		type CreateProvider,
		type RateLimits,
		type SoftLimitConfig,
		type UpdateAccount,
		type UpdateProvider,
	} from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { ghreviewUrl } from '$lib/config';
	import { safeHref } from '$lib/safeHref';
	import {
		isStaticCredential,
		providerFamily,
		providerLabel,
		PROVIDER_KINDS,
		type ProviderKind
	} from '$lib/providers';
	import SoftLimit from '$lib/components/molecules/SoftLimit.svelte';
	import { editorWindowKeys, isUsdKey } from '$lib/components/molecules/usage-windows';
	import AccountCard from '$lib/components/organisms/AccountCard.svelte';
	import GithubConnectors from '$lib/components/organisms/GithubConnectors.svelte';
	import DispatchersPanel from '$lib/components/organisms/DispatchersPanel.svelte';
	import ProviderSettingsList from '$lib/components/organisms/ProviderSettingsList.svelte';
	import FireworksProviderEditor from '$lib/components/organisms/FireworksProviderEditor.svelte';
	import AnthropicProviderEditor from '$lib/components/organisms/AnthropicProviderEditor.svelte';
	import FreeFormEnvEditor from '$lib/components/organisms/FreeFormEnvEditor.svelte';
	import {
		Button,
		Cluster,
		Container,
		Field,
		Heading,
		Input,
		Link,
		Modal,
		Select,
		Tabs,
		Text,
		type TabItem
	} from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	// Accounts is the single home for everything external: AI provider
	// accounts, GitHub connectors, and dispatchers. The Connectors tab only
	// appears when the ghreview review backend is deployed (`ghreviewUrl`).
	const reviewConfigured = $derived(ghreviewUrl() !== null);
	let tab = $state('ai');
	const tabs = $derived<TabItem[]>([
		{ id: 'ai', label: m.accounts_tab_ai() },
		...(reviewConfigured ? [{ id: 'connectors', label: m.accounts_tab_connectors() }] : []),
		{ id: 'dispatchers', label: m.accounts_tab_dispatchers() }
	]);

	const accounts = useAccounts();
	const actions = useAccountActions();
	// Accounts are user-owned; the admin token has no user identity, so an
	// admin operator picks the owning user explicitly.
	const me = useMe();
	const isAdmin = $derived(me.data?.role === 'admin');
	const users = useUsers(() => isAdmin);
	const activeUsers = $derived((users.data ?? []).filter((u) => !u.revoked_at));
	let ownerId = $state('');
	// Default the owner select to the first active user once loaded.
	$effect(() => {
		if (isAdmin && !ownerId && activeUsers.length) ownerId = activeUsers[0].id;
	});
	const guard = (p: Promise<unknown>) => p.catch((e: Error) => toasts.err(e.message));

	// ------------------------------------------------------------------------
	// Editor state. One modal, four modes:
	//   create        — new identity + its first provider credential
	//   add-provider  — attach a credential to an existing identity
	//   edit-account  — identity fields: name + write-only extra env
	//   edit-provider — one credential: aliases, soft limits, settings,
	//                   compatible-endpoint config, reauth, move
	type EditorMode = 'create' | 'add-provider' | 'edit-account' | 'edit-provider';
	let editor = $state<{ mode: EditorMode; accountId?: string; providerId?: string } | null>(null);

	const rows = $derived([...(accounts.data ?? [])]);
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
	// Anthropic's gateway-side request shaping, kept apart from `acctSettings`:
	// that one is the injected settings.json the harness reads, this one never
	// leaves the server.
	let anthropicSettings = $state<Record<string, unknown>>({});

	// Live windows for the provider under edit, so newly discovered (e.g.
	// model-scoped) windows appear in the editor automatically.
	const editorUsage = useAccountUsage(
		() => editingProvider?.id ?? '',
		() =>
			!!editingProvider &&
			(editingProvider.provider === 'anthropic' ||
				editingProvider.provider === 'openai' ||
				editingProvider.provider === 'fireworks')
	);
	const editorWindows = $derived(editorUsage.data?.windows ?? []);
	// Window keys to offer: baseline + observed + already-configured.
	const editorRows = $derived(
		editor?.mode === 'create' || editor?.mode === 'add-provider' || editor?.mode === 'edit-provider'
			? editorWindowKeys(
					editorWindows,
					editingProvider?.soft_limits ?? null,
					isFireworks ? 'fireworks' : (editingProvider?.family ?? null)
				)
			: []
	);
	// Ensure every offered key has an edit slot (seeded null; open* seeds configured).
	$effect(() => {
		for (const { key } of editorRows) {
			if (!(key in softEdits)) softEdits[key] = { cap: null, capUsd: null, bypass: null };
		}
	});

	// Per-provider settings + per-account env. `settings`
	// mirrors the provider's settings_json; `envRows` feed the identity's
	// write-only env_json (never read back, so they start empty on edit);
	// `replaceEnv` gates whether env_json is sent at all.
	let acctSettings = $state<Record<string, unknown>>({});
	let acctEnvRows = $state<{ name: string; value: string }[]>([]);
	let acctReplaceEnv = $state(false);
	let acctEnvRemove = $state<string[]>([]);
	// Move-provider target (merge path): another account of the same owner.
	let moveTarget = $state('');

	/** Normalise a soft-limit input: empty ⇒ null, else a clamped non-negative
	 *  integer. Tolerates either the number a number-input binds or a stray string. */
	function softNum(v: number | string | null | undefined): number | null {
		if (v === null || v === undefined || v === '') return null;
		const n = Math.round(Number(v));
		return Number.isFinite(n) ? Math.max(0, n) : null;
	}

	/** Same, for a dollar cap: money keeps its cents. */
	function softUsd(v: number | string | null | undefined): number | null {
		if (v === null || v === undefined || v === '') return null;
		const n = Number(v);
		return Number.isFinite(n) ? Math.max(0, n) : null;
	}

	/** The soft-limit map to send on save. Always sent as the whole
	 *  replacement map so clearing a window's cap/bypass sticks; windows with
	 *  neither cap nor bypass are dropped from the map. */
	function softLimits(): Record<string, SoftLimitConfig> {
		const out: Record<string, SoftLimitConfig> = {};
		for (const [key, v] of Object.entries(softEdits)) {
			const bypass = softNum(v.bypass);
			if (isUsdKey(key)) {
				const capUsd = softUsd(v.capUsd);
				if (capUsd !== null || bypass !== null)
					out[key] = { cap_usd: capUsd, bypass_minutes: bypass };
				continue;
			}
			const cap = softNum(v.cap);
			if (cap !== null || bypass !== null) out[key] = { cap_pct: cap, bypass_minutes: bypass };
		}
		return out;
	}

	/** The rate-limit object to send on save. Always sent as the whole
	 *  replacement so clearing a dimension sticks; a blank/zero dimension is
	 *  omitted (server reads that as unlimited). */
	function rateLimits(): RateLimits {
		return { rpm: softNum(rateEdits.rpm), tpm: softNum(rateEdits.tpm) };
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

	/** Trimmed catalog rows for the fireworks payloads; an entry without a model
	 *  id is dropped, and its label defaults to the id. */
	function fwModelList(): AccountModel[] {
		return fwModels
			.map((r) => ({ ...r, model: r.model.trim(), label: r.label.trim() || r.model.trim() }))
			.filter((r) => r.model);
	}

	/** Trimmed model rows for the compatible-endpoint payloads. */
	function modelList() {
		return modelRows
			.map((r) => ({ model: r.model.trim(), label: r.label.trim() || r.model.trim() }))
			.filter((r) => r.model);
	}

	// "Sign in with Claude" OAuth flow state.
	let oauthNonce = $state<string | null>(null);
	let oauthCode = $state('');
	let oauthBusy = $state(false);
	let showAdvanced = $state(false);
	// Reauth mode: editing an existing native provider to refresh its
	// rejected credentials, which reveals the sign-in block inside the edit modal.
	let reauthing = $state(false);
	// OAuth attach target: when adding/reauthenticating, finish the flow
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
		softEdits = {};
		rateEdits = { rpm: null, tpm: null };
		oauthNonce = null;
		oauthCode = '';
		oauthBusy = false;
		showAdvanced = false;
		reauthing = false;
		oauthAttachAccountId = null;
		acctSettings = {};
		anthropicSettings = {};
		acctEnvRows = [];
		acctReplaceEnv = false;
		acctEnvRemove = [];
		moveTarget = '';
		fwSettings = {};
		fwModels = [];
	}

	// Start the authorize leg: ask the server for an authorize URL, open it in a
	// new tab, and reveal the paste field. Works for both Claude (anthropic) and
	// "Sign in with ChatGPT" for Codex (openai).
	async function startOAuthLogin() {
		if (isAdmin && !ownerId && !oauthAttachAccountId) {
			toasts.err(m.accounts_err_pick_owner());
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
			toasts.err(errMessage(e));
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
			toasts.err(m.accounts_err_name_required());
			return;
		}
		if (!oauthNonce || !oauthCode.trim()) {
			toasts.err(
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
			toasts.err(errMessage(e));
		} finally {
			oauthBusy = false;
		}
	}

	function openCreate() {
		resetForm();
		editor = { mode: 'create' };
	}

	/** Provider kinds this account can still add: at most one per family
	 *  (anthropic/openai), mirroring the server's unique index. */
	function availableKinds(a: OAuthAccount): ProviderKind[] {
		const taken = new Set(a.providers.map((p) => p.family));
		return PROVIDER_KINDS.map((k) => k.value).filter((v) => !taken.has(providerFamily(v)));
	}

	function openAddProvider(a: OAuthAccount) {
		resetForm();
		editor = { mode: 'add-provider', accountId: a.id };
		provider = availableKinds(a)[0] ?? 'anthropic';
		ownerId = a.user_id;
		// The native OAuth flows attach via oauth/start's account_id.
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
		// Compatible endpoints can edit their model list in place. The
		// base URL, credential, and scheme are never read back, so they start
		// blank/"keep" — supplying one overwrites, leaving it keeps the stored value.
		if (p.provider === 'fireworks') {
			fwSettings = { ...(p.provider_settings ?? {}) };
			fwModels = (p.models ?? []).map((mo) => ({ ...mo }));
			authScheme = 'keep';
		} else if (p.provider.endsWith('-compatible')) {
			const ms = p.models ?? [];
			modelRows = ms.length
				? ms.map((m) => ({ model: m.model, label: m.label }))
				: [{ model: '', label: '' }];
			authScheme = 'keep';
		}
		// Aliases are editable for every provider.
		aliasRows = Object.entries(p.model_aliases ?? {}).map(([alias, model]) => ({ alias, model }));
		// Soft limits are editable per window for every provider.
		softEdits = {};
		for (const [key, v] of Object.entries(p.soft_limits ?? {})) {
			softEdits[key] = {
				cap: v.cap_pct ?? null,
				capUsd: v.cap_usd ?? null,
				bypass: v.bypass_minutes ?? null
			};
		}
		// Settings are editable per provider.
		acctSettings = { ...(p.settings_json ?? {}) };
		anthropicSettings = { ...(p.provider_settings ?? {}) };
		rateEdits = { rpm: p.rate_limits?.rpm ?? null, tpm: p.rate_limits?.tpm ?? null };
	}

	// Reauthenticate a flagged provider: open its edit modal, flip into
	// reauth mode (reveals the sign-in block), and kick the authorize leg. The
	// pasted code is exchanged by finishOAuthLogin, which refreshes the
	// same-family credential in place and clears `needs_reauth` server-side.
	function reauth(a: OAuthAccount, p: AccountProvider) {
		openEditProvider(a, p);
		ownerId = a.user_id;
		reauthing = true;
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
		const model_aliases = aliasObject();
		try {
			if (mode === 'edit-account' && editor?.accountId) {
				if (!name.trim()) {
					toasts.err(m.accounts_err_name_required());
					return;
				}
				const identity: UpdateAccount = { name: name.trim() };
				if (acctReplaceEnv) identity.env_json = envObject();
				else if (acctEnvRemove.length) identity.env_remove = acctEnvRemove;
				await actions.update(editor.accountId, identity);
				toasts.ok(m.accounts_account_updated());
			} else if (mode === 'edit-provider' && editor?.accountId && editor.providerId) {
				// Always send the alias map + soft limits + settings so clearing
				// them sticks (empty object clears the stored blob). Settings only
				// apply to the claude-code harness → anthropic-family providers.
				const body: UpdateProvider = {
					model_aliases,
					soft_limits: softLimits(),
					rate_limits: rateLimits(),
					...(editingProvider?.family === 'anthropic'
						? { settings_json: acctSettings, provider_settings: anthropicSettings }
						: {})
				};
				if (isFireworks) {
					body.models = fwModelList();
					body.provider_settings = fwSettings;
					if (baseUrl.trim()) body.base_url = baseUrl.trim();
					if (credential.trim()) body.access_token = credential.trim();
				} else if (isCompatible) {
					body.models = modelList();
					if (baseUrl.trim()) body.base_url = baseUrl.trim();
					if (credential.trim()) body.access_token = credential.trim();
					if (authScheme !== 'keep') body.auth_scheme = authScheme;
				}
				await actions.updateProvider(editor.accountId, editor.providerId, body);
				toasts.ok(m.accounts_provider_updated());
			} else if (mode === 'add-provider' && editor?.accountId) {
				// Native OAuth adds go through finishOAuthLogin instead; this path is
				// the compatible-endpoint / pasted-refresh-token attach.
				const spec: CreateProvider = {
					provider,
					...(Object.keys(model_aliases).length ? { model_aliases } : {}),
					soft_limits: softLimits(),
					rate_limits: rateLimits()
				};
				if (isFireworks) {
					spec.auth_scheme = authScheme === 'keep' ? 'bearer' : authScheme;
					if (baseUrl.trim()) spec.base_url = baseUrl.trim();
					if (credential.trim()) spec.access_token = credential.trim();
					const models = fwModelList();
					if (models.length) spec.models = models;
					if (Object.keys(fwSettings).length) spec.provider_settings = fwSettings;
				} else if (isCompatible) {
					if (!baseUrl.trim()) {
						toasts.err(m.accounts_err_base_url_required());
						return;
					}
					spec.base_url = baseUrl.trim();
					spec.auth_scheme = authScheme === 'keep' ? 'bearer' : authScheme;
					if (credential.trim()) spec.access_token = credential.trim();
					const models = modelList();
					if (models.length) spec.models = models;
				} else {
					if (!refreshToken.trim()) {
						toasts.err(m.accounts_err_refresh_token_required());
						return;
					}
					spec.refresh_token = refreshToken.trim();
				}
				await actions.addProvider(editor.accountId, spec);
				toasts.ok(m.accounts_provider_added());
			} else {
				// create: identity + first credential in one call.
				if (!name.trim()) {
					toasts.err(m.accounts_err_name_required());
					return;
				}
				if (isAdmin && !ownerId) {
					toasts.err(m.accounts_err_pick_owner());
					return;
				}
				let body: CreateAccount;
				if (isFireworks) {
					const models = fwModelList();
					body = {
						name: name.trim(),
						provider,
						auth_scheme: authScheme === 'keep' ? 'bearer' : authScheme,
						...(baseUrl.trim() ? { base_url: baseUrl.trim() } : {}),
						...(credential.trim() ? { access_token: credential.trim() } : {}),
						...(models.length ? { models } : {}),
						...(Object.keys(fwSettings).length ? { provider_settings: fwSettings } : {}),
						...(Object.keys(model_aliases).length ? { model_aliases } : {}),
						soft_limits: softLimits(),
						rate_limits: rateLimits(),
						...(isAdmin ? { user_id: ownerId } : {})
					};
				} else if (isCompatible) {
					if (!baseUrl.trim()) {
						toasts.err(m.accounts_err_base_url_required());
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
						soft_limits: softLimits(),
						rate_limits: rateLimits(),
						...(isAdmin ? { user_id: ownerId } : {}),
					};
				} else {
					if (!refreshToken.trim()) {
						toasts.err(m.accounts_err_refresh_token_required());
						return;
					}
					body = {
						name: name.trim(),
						provider,
						refresh_token: refreshToken.trim(),
						...(Object.keys(model_aliases).length ? { model_aliases } : {}),
						soft_limits: softLimits(),
						rate_limits: rateLimits(),
						...(isAdmin ? { user_id: ownerId } : {}),
					};
				}
				await actions.create(body);
				toasts.ok(m.accounts_account_added());
			}
			close();
		} catch (e) {
			toasts.err(errMessage(e));
		}
	}

	function removeAccount(a: OAuthAccount) {
		if (!confirm(m.accounts_confirm_delete_account({ name: a.name }))) return;
		guard(actions.remove(a.id).then(() => toasts.ok(m.accounts_deleted())));
	}

	function removeProvider(a: OAuthAccount, p: AccountProvider) {
		if (
			!confirm(
				m.accounts_confirm_remove_provider({ provider: providerLabel(p.provider), name: a.name })
			)
		)
			return;
		guard(actions.removeProvider(a.id, p.id).then(() => toasts.ok(m.accounts_provider_removed())));
	}

	async function moveProvider() {
		if (!editor?.accountId || !editor.providerId || !moveTarget) return;
		try {
			await actions.moveProvider(editor.accountId, editor.providerId, moveTarget);
			toasts.ok(m.accounts_provider_moved());
			close();
		} catch (e) {
			toasts.err(errMessage(e));
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
			? m.accounts_modal_new_account()
			: editor?.mode === 'add-provider'
				? m.accounts_modal_add_provider({ name: editingAccount?.name ?? '' })
				: editor?.mode === 'edit-account'
					? m.accounts_modal_edit_account()
					: reauthing
						? m.accounts_modal_reauth()
						: m.accounts_modal_edit_provider({ provider: providerLabel(editingProvider?.provider ?? '') })
	);
</script>

<!-- Accounts rows are full-width composites (provider boxes side by side), so
     the screen breaks out of the app's prose-width shell and re-centres on the
     wider measure. Head + tabs + rows share the wrapper so they stay aligned. -->
<Container fullWidth as="div">
<div class="accounts-wide">
	<div class="page-head"><Heading level={1}>{m.accounts_title()}</Heading></div>

	<Tabs {tabs} bind:value={tab} label={m.accounts_sections_label()}>
		{#snippet panel(id)}
			{#if id === 'ai'}
				<div class="ai-pane">
				<Cluster class="acct-bar" justify="space-between" align="center" gap="var(--sp-3)">
					<Text as="p" tone="muted" size="sm" class="intro">
						{m.accounts_ai_intro()}
					</Text>
					<Button control variant="primary" onclick={openCreate}>{m.accounts_new_account()}</Button>
				</Cluster>

				{#if accounts.isLoading}
					<div class="empty"><span class="spin"></span></div>
				{:else if rows.length === 0}
					<div class="empty"><Text tone="muted">{m.accounts_empty()}</Text></div>
				{:else}
					<div class="acct-list">
						{#each rows as a (a.id)}
							<AccountCard
								account={a}
								enabled={tab === 'ai'}
								managed={isManaged(a)}
								canAddProvider={!isManaged(a) && availableKinds(a).length > 0}
								canShare={!isManaged(a) && (isAdmin || a.user_id === me.data?.user_id)}
								showOwner={isAdmin}
								onedit={() => openEditAccount(a)}
								onremove={() => removeAccount(a)}
								onaddprovider={() => openAddProvider(a)}
								oneditprovider={(p) => openEditProvider(a, p)}
								onreauthprovider={(p) => reauth(a, p)}
								onremoveprovider={(p) => removeProvider(a, p)}
							/>
						{/each}
					</div>
				{/if}
				</div>
			{:else if id === 'connectors'}
				<GithubConnectors />
			{:else if id === 'dispatchers'}
				<DispatchersPanel heading={false} />
			{/if}
		{/snippet}
	</Tabs>
</div>
</Container>

{#if editor !== null}
	<Modal title={modalTitle} onclose={close} size="lg" resizeKey="account-editor">
		{#snippet body()}
			<div class="editor-body">
				{#if editor?.mode === 'create' || editor?.mode === 'edit-account'}
					<Field label={m.accounts_field_name()}>
						<Input bind:value={name} placeholder={m.accounts_field_name_placeholder()} />
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
						{@const isEdit = editor?.mode === 'edit-provider'}
						<Field label={m.accounts_field_credential()}>
							<Input
								type="password"
								bind:value={credential}
								placeholder={isEdit
									? m.accounts_placeholder_keep_current()
									: 'fw_...'}
							/>
						</Field>
						<Field label={m.accounts_field_base_url()}>
							<Input
								bind:value={baseUrl}
								placeholder={isEdit
									? m.accounts_placeholder_keep_current()
									: 'https://api.fireworks.ai/inference/v1'}
							/>
						</Field>
						<FireworksProviderEditor bind:settings={fwSettings} bind:models={fwModels} />
					{:else if isCompatible}
						<!-- Compatible endpoint: base URL + a static credential +
						     a model list. No OAuth; the gateway forwards the credential and
						     skips refresh. On edit the model list is editable in
						     place; base URL / credential / scheme are write-only — blank or
						     "keep" leaves the stored value untouched. -->
						{@const isEdit = editor?.mode === 'edit-provider'}
						<Field label={m.accounts_field_base_url()}>
							<Input
								bind:value={baseUrl}
								placeholder={isEdit ? m.accounts_placeholder_keep_current() : 'https://litellm.example/v1'}
							/>
						</Field>
						<Field label={m.accounts_field_auth_scheme()}>
							<Select bind:value={authScheme}>
								{#if isEdit}
									<option value="keep">{m.accounts_auth_keep()}</option>
								{/if}
								<option value="bearer">{m.accounts_auth_bearer()}</option>
								<option value="api_key">{m.accounts_auth_api_key()}</option>
							</Select>
						</Field>
						<Field label={m.accounts_field_credential()}>
							<Input
								type="password"
								bind:value={credential}
								placeholder={isEdit
									? m.accounts_placeholder_keep_current()
									: m.accounts_placeholder_credential()}
							/>
						</Field>
						<div class="models">
							<Text as="div" tone="muted" size="sm">{m.accounts_models_label()}</Text>
							{#each modelRows as row, i (i)}
								<div class="model-row">
									<Input bind:value={row.model} placeholder={m.accounts_placeholder_model_code()} />
									<Input bind:value={row.label} placeholder={m.accounts_placeholder_model_label()} />
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
					{:else if editor?.mode === 'create' || editor?.mode === 'add-provider' || reauthing}
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
						{#if !reauthing}
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
					{#if editor?.mode === 'edit-provider' || isCompatible}
						<div class="models">
							<Text as="div" tone="muted" size="sm">{m.accounts_aliases_label()}</Text>
							<Text as="div" tone="faint" size="xs">
								{m.accounts_aliases_help()}
							</Text>
							{#each aliasRows as row, i (i)}
								<div class="model-row">
									<Input bind:value={row.alias} placeholder={m.accounts_placeholder_alias_name()} />
									<Input bind:value={row.model} placeholder={m.accounts_placeholder_alias_model()} />
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
					{#if provider === 'anthropic' || provider === 'anthropic-compatible' || provider === 'openai' || isFireworks}
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
										observed={false}
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

					<!-- Per-provider settings. Only the claude-code
					     harness has an injectable settings.json today, so only
					     anthropic-family providers get the toggle list. -->
					{#if editor?.mode === 'edit-provider' && editingProvider}
						{#if editingProvider.family === 'anthropic'}
							<ProviderSettingsList bind:settings={acctSettings} />
							<AnthropicProviderEditor bind:settings={anthropicSettings} />
						{:else if editingProvider.family === 'openai'}
							<Text tone="faint" size="sm">
								{m.accounts_no_codex_settings()}
							</Text>
						{/if}

						<!-- Move: re-parent this credential onto another account of
						     the same owner — the merge path for migrated split rows. -->
						{#if !reauthing && moveTargets.length}
							<div class="models">
								<Text as="div" tone="muted" size="sm">{m.accounts_move_label()}</Text>
								<Text as="div" tone="faint" size="xs">
									{m.accounts_move_help({ family: editingProvider.family })}
								</Text>
								<div class="move-row">
									<Select bind:value={moveTarget}>
										<option value="">{m.accounts_move_pick()}</option>
										{#each moveTargets as t (t.id)}
											<option value={t.id}>{t.name}</option>
										{/each}
									</Select>
									<Button disabled={!moveTarget} onclick={moveProvider}>{m.accounts_move_button()}</Button>
								</div>
							</div>
						{/if}
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

<style>
	.page-head {
		margin-bottom: var(--sp-3);
	}
	.ai-pane :global(.acct-bar) {
		margin-bottom: var(--sp-3);
	}
	/* Intro copy shares the header row with the New-account button; it's passed
	   to a Text atom, so reach it through the atom's rendered element. */
	.ai-pane :global(.acct-bar .intro) {
		max-width: 60ch;
	}
	/* Re-centre on the wide measure after Container's full-bleed break-out. */
	.accounts-wide {
		width: 100%;
		max-width: var(--content-wide);
		margin-inline: auto;
	}
	/* One account per row; each row owns its own internal box layout. */
	.acct-list {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
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
