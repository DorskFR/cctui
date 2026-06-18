<script lang="ts">
	import {
		useAccounts,
		useAccountActions,
		useMe,
		useUsers,
		type OAuthAccount,
		type CreateAccount,
	} from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { compact } from '$lib/format';
	import UsageBars from '$lib/components/molecules/UsageBars.svelte';
	import AdapterIcon from '$lib/components/atoms/AdapterIcon.svelte';
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
		Text,
		Timestamp
	} from '@dorsk/tsumikit';
	import { providerLabel } from './accounts.logic';

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
	let authScheme = $state<'bearer' | 'api_key'>('bearer');
	let modelRows = $state<{ model: string; label: string }[]>([{ model: '', label: '' }]);
	const isCompatible = $derived(provider.endsWith('-compatible'));

	// "Sign in with Claude" OAuth flow state (CCT-243).
	let oauthNonce = $state<string | null>(null);
	let oauthCode = $state('');
	let oauthBusy = $state(false);
	let showAdvanced = $state(false);

	function resetForm() {
		name = '';
		provider = 'anthropic';
		refreshToken = '';
		baseUrl = '';
		credential = '';
		authScheme = 'bearer';
		modelRows = [{ model: '', label: '' }];
		oauthNonce = null;
		oauthCode = '';
		oauthBusy = false;
		showAdvanced = false;
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
			const r = await actions.oauthStart(provider, isAdmin ? ownerId : undefined);
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

	function openRename(a: OAuthAccount) {
		resetForm();
		editing = a.id;
		name = a.name;
		provider = (a.provider as typeof provider) ?? 'anthropic';
	}

	function close() {
		editing = undefined;
	}

	async function save() {
		if (!name.trim()) {
			toasts.err('Name is required');
			return;
		}
		try {
			if (editing) {
				await actions.rename(editing, name.trim());
				toasts.ok('Account renamed');
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
</script>

<Cluster as="header" class="bar" justify="space-between" align="center">
	<Heading level={1}>Accounts</Heading>
	<Button control variant="primary" onclick={openCreate}>+ New account</Button>
</Cluster>

<div class="intro">
	<Text as="p" tone="muted" size="sm">
		Named OAuth accounts for Claude and Codex. Pick one per job at spawn time; the
		session runs through a passthrough gateway under that account. Tokens are
		stored encrypted and never shown again.
	</Text>
</div>

{#if $accounts.isLoading}
	<div class="empty"><span class="spin"></span></div>
{:else if rows.length === 0}
	<div class="empty"><Text tone="muted">No accounts yet.</Text></div>
{:else}
	<AutoGrid min="22rem" gap="var(--sp-3)">
		{#each rows as a (a.id)}
			<Card class="account-card">
				<Stack gap="var(--sp-3)" class="card-body">
					<Cluster gap="var(--sp-2)" align="center" wrap={false}>
						<span class="provider-mark" title={providerLabel(a.provider)}>
							<AdapterIcon provider={a.provider} size={22} />
						</span>
						<Heading level={2} size="lg" class="account-name">{a.name}</Heading>
					</Cluster>
					{#if a.provider === 'anthropic'}
						<div class="usage-block">
							<Text as="div" tone="muted" size="xs" class="usage-head">Subscription usage</Text>
							<UsageBars id={a.id} provider={a.provider} />
						</div>
					{/if}
					<dl class="stats">
						{#if isAdmin}
							<div><dt>Owner</dt><dd>{a.user_name ?? '—'}</dd></div>
						{/if}
						<div><dt>Requests</dt><dd>{compact(a.request_count)}</dd></div>
						<div><dt>Last used</dt><dd><Timestamp value={a.last_used_at} mode="relative" tone="inherit" /></dd></div>
						<div><dt>Created</dt><dd><Timestamp value={a.created_at} mode="date" tone="inherit" /></dd></div>
					</dl>
				</Stack>
				<Cluster as="footer" gap="var(--sp-1)" justify="flex-end" class="card-foot">
					{#if a.managed}
						<Text tone="faint" size="xs">Managed (read-only)</Text>
					{:else}
						<Button size="sm" onclick={() => openRename(a)}>Rename</Button>
						<Button size="sm" variant="danger" onclick={() => remove(a)}>Delete</Button>
					{/if}
				</Cluster>
			</Card>
		{/each}
	</AutoGrid>
{/if}

{#if editing !== undefined}
	<Modal title={editing ? 'Rename account' : 'New account'} onclose={close}>
		{#snippet body()}
			<div class="editor-body">
				<Field label="Name">
					<Input bind:value={name} placeholder="personal" />
				</Field>
				{#if !editing}
					{#if isAdmin}
						<Field label="Owner">
							<Select bind:value={ownerId}>
								{#each activeUsers as u (u.id)}
									<option value={u.id}>{u.name}</option>
								{/each}
							</Select>
						</Field>
					{/if}
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

					{#if isCompatible}
						<!-- Compatible endpoint (CCT-399): base URL + a static credential +
						     a model list. No OAuth; the gateway forwards the credential and
						     skips refresh. -->
						<Field label="Base URL">
							<Input bind:value={baseUrl} placeholder="https://litellm.example/v1" />
						</Field>
						<Field label="Auth scheme">
							<Select bind:value={authScheme}>
								<option value="bearer">Bearer token</option>
								<option value="api_key">API key</option>
							</Select>
						</Field>
						<Field label="Credential (optional)">
							<Input
								type="password"
								bind:value={credential}
								placeholder="bearer / API key (blank for an open proxy)"
							/>
						</Field>
						<div class="models">
							<Text as="div" tone="muted" size="sm">Models</Text>
							{#each modelRows as row, i (i)}
								<div class="model-row">
									<Input bind:value={row.model} placeholder="model code (e.g. qwen3-coder)" />
									<Input bind:value={row.label} placeholder="label (optional)" />
									<Button
										size="sm"
										variant="danger"
										onclick={() => (modelRows = modelRows.filter((_, j) => j !== i))}
										disabled={modelRows.length === 1}>✕</Button
									>
								</div>
							{/each}
							<Button
								size="sm"
								onclick={() => (modelRows = [...modelRows, { model: '', label: '' }])}
								>+ Add model</Button
							>
						</div>
					{:else}
						<!-- Sign in with Claude / ChatGPT: authorize upstream, paste back. -->
						{#if !oauthNonce}
							<Button
								size="sm"
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
				{/if}
			</div>
		{/snippet}
		{#snippet footer()}
			<div class="spacer"></div>
			<Button size="sm" onclick={close}>Cancel</Button>
			{#if !editing && oauthNonce && !showAdvanced}
				<Button size="sm" variant="primary" disabled={oauthBusy} onclick={finishOAuthLogin}>Save</Button>
			{:else}
				<Button size="sm" variant="primary" onclick={save}>Save</Button>
			{/if}
		{/snippet}
	</Modal>
{/if}

<style>
	:global(.bar) {
		margin-bottom: var(--sp-2);
	}
	/* Typography from the Text atom; only the page rhythm lives here. */
	.intro {
		margin-bottom: var(--sp-4);
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
	.usage-block {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-3);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		background: var(--bg-elevated-2);
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
