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
	import { dateOnly, relativeTime, compact } from '$lib/format';
	import UsageChip from '$lib/components/UsageChip.svelte';

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
	let provider = $state<'anthropic' | 'openai'>('anthropic');
	let refreshToken = $state('');

	// "Sign in with Claude" OAuth flow state (CCT-243).
	let oauthNonce = $state<string | null>(null);
	let oauthCode = $state('');
	let oauthBusy = $state(false);
	let showAdvanced = $state(false);

	function resetForm() {
		name = '';
		provider = 'anthropic';
		refreshToken = '';
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
		provider = (a.provider as 'anthropic' | 'openai') ?? 'anthropic';
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
				if (!refreshToken.trim()) {
					toasts.err('Refresh token is required');
					return;
				}
				if (isAdmin && !ownerId) {
					toasts.err('Pick the owning user first');
					return;
				}
				const body: CreateAccount = {
					name: name.trim(),
					provider,
					refresh_token: refreshToken.trim(),
					...(isAdmin ? { user_id: ownerId } : {}),
				};
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

	const providerLabel = (p: string) =>
		p === 'anthropic' ? 'Claude' : p === 'openai' ? 'Codex' : p;

	// Estimated cost (CCT-273): sub-cent → "<$0.01", small → 2 dp, large → compact.
	const usd = (v: number) =>
		!v ? '$0' : v < 0.01 ? '<$0.01' : v < 1000 ? `$${v.toFixed(2)}` : `$${compact(v)}`;
</script>

<div class="bar row">
	<h1 class="page-title">Accounts</h1>
	<div class="spacer"></div>
	<button class="btn-control btn-primary" onclick={openCreate}>+ New account</button>
</div>

<p class="hint">
	Named OAuth accounts for Claude and Codex. Pick one per job at spawn time; the
	session runs through a passthrough gateway under that account. Tokens are
	stored encrypted and never shown again.
</p>

{#if $accounts.isLoading}
	<div class="empty"><span class="spin"></span></div>
{:else if rows.length === 0}
	<div class="empty">No accounts yet.</div>
{:else}
	<div class="accounts-grid">
		{#each rows as a (a.id)}
			<article class="card account-card">
				<div class="account-head row">
					<div class="account-title">
						<h2>{a.name}</h2>
						<div class="muted sm">{providerLabel(a.provider)}</div>
					</div>
					<span class="badge">{providerLabel(a.provider)}</span>
				</div>
				<div class="account-fields">
					{#if isAdmin}
						<div class="field-chip"><span>Owner</span><b>{a.user_name ?? '—'}</b></div>
					{/if}
					<div class="field-chip usage"><span>Usage</span><UsageChip id={a.id} provider={a.provider} /></div>
					<div class="field-chip"><span>Requests</span><b>{compact(a.request_count)}</b></div>
					<div class="field-chip"><span>Tokens</span><b>{compact(a.total_tokens)}</b></div>
					<div class="field-chip"><span>Cost</span><b>{usd(a.est_cost_usd)}</b></div>
					<div class="field-chip"><span>Last used</span><b>{relativeTime(a.last_used_at)}</b></div>
					<div class="field-chip"><span>Created</span><b>{dateOnly(a.created_at)}</b></div>
				</div>
				<div class="row acts">
					<button class="btn btn-sm" onclick={() => openRename(a)}>Rename</button>
					<button class="btn btn-sm btn-danger" onclick={() => remove(a)}>Delete</button>
				</div>
			</article>
		{/each}
	</div>
{/if}

{#if editing !== undefined}
	<div
		class="overlay"
		role="presentation"
		onclick={close}
		onkeydown={(e) => e.key === 'Escape' && close()}
	>
		<div
			class="card editor"
			role="dialog"
			tabindex="-1"
			onclick={(e) => e.stopPropagation()}
			onkeydown={(e) => e.stopPropagation()}
		>
			<h2>{editing ? 'Rename account' : 'New account'}</h2>
			<label class="fld">
				<span>Name</span>
				<input class="input" bind:value={name} placeholder="personal" />
			</label>
			{#if !editing}
				{#if isAdmin}
					<label class="fld">
						<span>Owner</span>
						<select class="input" bind:value={ownerId}>
							{#each activeUsers as u (u.id)}
								<option value={u.id}>{u.name}</option>
							{/each}
						</select>
					</label>
				{/if}
				<label class="fld">
					<span>Provider</span>
					<select
						class="input"
						bind:value={provider}
						onchange={() => {
							oauthNonce = null;
							oauthCode = '';
						}}
					>
						<option value="anthropic">Claude (anthropic)</option>
						<option value="openai">Codex (openai)</option>
					</select>
				</label>
				<!-- Sign in with Claude / ChatGPT: authorize upstream, paste back. -->
				{#if !oauthNonce}
					<button
						class="btn btn-sm btn-primary signin"
						disabled={oauthBusy}
						onclick={startOAuthLogin}
					>
						{oauthBusy
							? 'Opening…'
							: provider === 'openai'
								? 'Sign in with ChatGPT'
								: 'Sign in with Claude'}
					</button>
				{:else}
					<label class="fld">
						<span>{provider === 'openai' ? 'URL from ChatGPT' : 'Code from claude.ai'}</span>
						<input
							class="input"
							bind:value={oauthCode}
							placeholder={provider === 'openai'
								? 'paste the http://localhost:1455/auth/callback?... URL'
								: 'paste the code#state shown after authorizing'}
						/>
					</label>
					{#if provider === 'openai'}
						<p class="hint sub">
							The browser tab will fail to load localhost:1455 — that's expected.
							Copy the full URL from its address bar and paste it above.
						</p>
					{/if}
					<p class="hint sub">
						Didn't get {provider === 'openai' ? 'a URL' : 'a code'}?
						<button class="linkbtn" onclick={startOAuthLogin}
							>Open {provider === 'openai' ? 'ChatGPT' : 'claude.ai'} again</button
						>
					</p>
				{/if}
				<details bind:open={showAdvanced} class="adv">
					<summary>Advanced: paste a refresh token instead</summary>
					<label class="fld">
						<span>OAuth refresh token</span>
						<input
							class="input"
							type="password"
							bind:value={refreshToken}
							placeholder="paste the OAuth refresh token"
						/>
					</label>
				</details>
			{/if}
			<div class="row editor-acts">
				<div class="spacer"></div>
				<button class="btn btn-sm" onclick={close}>Cancel</button>
				{#if !editing && oauthNonce && !showAdvanced}
					<button
						class="btn btn-sm btn-primary"
						disabled={oauthBusy}
						onclick={finishOAuthLogin}>Save</button
					>
				{:else}
					<button class="btn btn-sm btn-primary" onclick={save}>Save</button>
				{/if}
			</div>
		</div>
	</div>
{/if}

<style>
	.bar {
		margin-bottom: var(--sp-2);
	}
	.page-title {
		font-size: var(--fs-2xl);
	}
	.hint {
		color: var(--text-muted);
		font-size: var(--fs-sm);
		margin-bottom: var(--sp-4);
	}
	.accounts-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(min(100%, 22rem), 1fr));
		gap: var(--sp-3);
	}
	.account-card {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.account-head {
		align-items: flex-start;
	}
	.account-title {
		flex: 1;
		min-width: 0;
	}
	.account-title h2 {
		font-size: var(--fs-lg);
		line-height: var(--lh-tight);
		word-break: break-word;
	}
	.sm {
		font-size: var(--fs-xs);
	}
	.account-fields {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--sp-2);
	}
	.field-chip {
		min-width: 0;
		padding: var(--sp-2);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		background: var(--bg-elevated-2);
	}
	.field-chip span {
		display: block;
		color: var(--text-muted);
		font-size: var(--fs-xs);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}
	.field-chip b {
		display: block;
		margin-top: 0.1rem;
		font-size: var(--fs-sm);
		font-weight: var(--fw-medium);
		overflow-wrap: anywhere;
	}
	.field-chip.usage {
		grid-column: 1 / -1;
	}
	.acts {
		gap: var(--sp-1);
		justify-content: flex-end;
		flex-wrap: wrap;
	}
	.overlay {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.5);
		display: flex;
		align-items: center;
		justify-content: center;
		padding: var(--sp-4);
		z-index: 50;
	}
	.editor {
		width: 100%;
		max-width: 30rem;
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.fld {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		font-size: var(--fs-sm);
	}
	.fld span {
		color: var(--text-muted);
	}
	.editor-acts {
		gap: var(--sp-1);
		margin-top: var(--sp-2);
	}
	.signin {
		align-self: flex-start;
	}
	.hint.sub {
		margin: 0;
	}
	.linkbtn {
		background: none;
		border: none;
		padding: 0;
		color: var(--accent, var(--text));
		cursor: pointer;
		text-decoration: underline;
		font: inherit;
	}
	.adv summary {
		cursor: pointer;
		color: var(--text-muted);
		font-size: var(--fs-sm);
	}
	.adv .fld {
		margin-top: var(--sp-2);
	}
	@media (max-width: 639px) {
		.account-fields {
			grid-template-columns: 1fr;
		}
	}
</style>
