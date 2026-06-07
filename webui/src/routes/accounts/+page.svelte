<script lang="ts">
	import {
		useAccounts,
		useAccountActions,
		type OAuthAccount,
		type CreateAccount,
	} from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { dateOnly, relativeTime, compact } from '$lib/format';

	const accounts = useAccounts();
	const actions = useAccountActions();
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
	// new tab, and reveal the code-paste field.
	async function startClaudeLogin() {
		oauthBusy = true;
		try {
			const r = await actions.oauthStart('anthropic');
			oauthNonce = r.nonce;
			window.open(r.authorize_url, '_blank', 'noopener');
			toasts.ok('Opened claude.ai — authorize, then paste the code below');
		} catch (e) {
			toasts.err((e as Error).message);
		} finally {
			oauthBusy = false;
		}
	}

	// Finish: exchange the pasted code#state for tokens and create the account.
	async function finishClaudeLogin() {
		if (!name.trim()) {
			toasts.err('Name is required');
			return;
		}
		if (!oauthNonce || !oauthCode.trim()) {
			toasts.err('Paste the code from claude.ai first');
			return;
		}
		oauthBusy = true;
		try {
			await actions.oauthFinish({
				nonce: oauthNonce,
				name: name.trim(),
				code: oauthCode.trim(),
			});
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
				const body: CreateAccount = {
					name: name.trim(),
					provider,
					refresh_token: refreshToken.trim(),
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
</script>

<div class="bar row">
	<h1 class="page-title">Accounts</h1>
	<div class="spacer"></div>
	<button class="btn btn-primary btn-sm" onclick={openCreate}>+ New account</button>
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
	<div class="card table-card">
		<table class="disp">
			<thead>
				<tr>
					<th class="col-name">Name</th>
					<th class="col-prov">Provider</th>
					<th class="col-usage">Requests</th>
					<th class="col-usage">Bytes</th>
					<th class="col-used">Last used</th>
					<th class="col-created">Created</th>
					<th class="col-actions">Actions</th>
				</tr>
			</thead>
			<tbody>
				{#each rows as a (a.id)}
					<tr>
						<td class="col-name"><span class="name">{a.name}</span></td>
						<td class="col-prov"><span class="badge">{providerLabel(a.provider)}</span></td>
						<td class="col-usage faint">{compact(a.request_count)}</td>
						<td class="col-usage faint">{compact(a.bytes_transferred)}</td>
						<td class="col-used faint">{relativeTime(a.last_used_at)}</td>
						<td class="col-created faint">{dateOnly(a.created_at)}</td>
						<td class="col-actions">
							<div class="row acts">
								<button class="btn btn-sm" onclick={() => openRename(a)}>Rename</button>
								<button class="btn btn-sm btn-danger" onclick={() => remove(a)}>Delete</button>
							</div>
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
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
				<label class="fld">
					<span>Provider</span>
					<select class="input" bind:value={provider}>
						<option value="anthropic">Claude (anthropic)</option>
						<option value="openai">Codex (openai)</option>
					</select>
				</label>
				{#if provider === 'anthropic'}
					<!-- Sign in with Claude: authorize at claude.ai, paste the code. -->
					{#if !oauthNonce}
						<button
							class="btn btn-sm btn-primary signin"
							disabled={oauthBusy}
							onclick={startClaudeLogin}
						>
							{oauthBusy ? 'Opening…' : 'Sign in with Claude'}
						</button>
					{:else}
						<label class="fld">
							<span>Code from claude.ai</span>
							<input
								class="input"
								bind:value={oauthCode}
								placeholder="paste the code#state shown after authorizing"
							/>
						</label>
						<p class="hint sub">
							Didn't get a code?
							<button class="linkbtn" onclick={startClaudeLogin}
								>Open claude.ai again</button
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
				{:else}
					<label class="fld">
						<span>OAuth refresh token</span>
						<input
							class="input"
							type="password"
							bind:value={refreshToken}
							placeholder="paste the OAuth refresh token"
						/>
					</label>
				{/if}
			{/if}
			<div class="row editor-acts">
				<div class="spacer"></div>
				<button class="btn btn-sm" onclick={close}>Cancel</button>
				{#if !editing && provider === 'anthropic' && oauthNonce && !showAdvanced}
					<button
						class="btn btn-sm btn-primary"
						disabled={oauthBusy}
						onclick={finishClaudeLogin}>Save</button
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
	.table-card {
		padding: 0;
		overflow-x: auto;
	}
	table.disp {
		width: 100%;
		border-collapse: collapse;
		table-layout: fixed;
	}
	th {
		text-align: left;
		font-size: var(--fs-xs);
		color: var(--text-muted);
		text-transform: uppercase;
		letter-spacing: 0.04em;
		font-weight: var(--fw-semibold);
		padding: var(--sp-2) var(--sp-3);
		border-bottom: 1px solid var(--border);
	}
	td {
		padding: var(--sp-2) var(--sp-3);
		vertical-align: middle;
		border-top: 1px solid var(--border);
	}
	tbody tr:first-child td {
		border-top: none;
	}
	.name {
		font-weight: var(--fw-semibold);
	}
	.col-prov {
		width: 7rem;
	}
	.col-usage {
		width: 6rem;
	}
	.col-used {
		width: 8rem;
	}
	.col-created {
		width: 8rem;
	}
	.col-actions {
		width: 13rem;
	}
	.acts {
		gap: var(--sp-1);
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
	@media (max-width: 720px) {
		.col-created,
		.col-usage {
			display: none;
		}
	}
</style>
