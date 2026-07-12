<!--
  Dispatcher management, extracted from the /dispatchers route (CCT-403) so it
  can be hosted under the Accounts page (the single home for everything that
  connects to something external) as well as its own route.
-->
<script lang="ts">
	import {
		useUserDispatchers,
		useDispatcherActions,
		useAccounts,
		primaryProvider,
		type UserDispatcher,
	} from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { Badge, Button, Field, Heading, Input, Modal, Select, Text, Timestamp } from '@dorsk/tsumikit';
	import { livenessLabel, livenessTone } from '$lib/dispatchers.logic';
	import { m } from '$lib/paraglide/messages';

	// When embedded under Accounts the page already shows an <h1>, so the panel's
	// own heading drops a level; standalone it stays an <h1>.
	let { heading = true }: { heading?: boolean } = $props();

	const dispatchers = useUserDispatchers();
	const actions = useDispatcherActions();

	// Enroll editor state. `editing` holds the id when renaming, null when
	// enrolling fresh, undefined when the editor is closed. Declared before
	// `useAccounts` below: its `enabled` getter reads `editing` synchronously
	// at query creation, so the binding must already be initialized (a
	// later `let` would hit a temporal-dead-zone ReferenceError — CCT-428).
	let editing = $state<string | null | undefined>(undefined);

	// CCT-427: optional default OAuth account to bind at enroll. The picker is
	// only shown while enrolling (not renaming).
	const accounts = useAccounts(() => editing === null);
	const guard = (p: Promise<unknown>) => p.catch((e: Error) => toasts.err(e.message));

	let name = $state('');
	let kind = $state<'kubernetes' | 'docker' | 'http'>('kubernetes');
	// `"<name>\0<provider>"` of the bound default account, or '' for none.
	// The NUL separator can't appear in a name/provider, so the split is safe.
	let accountKey = $state('');

	// One-shot key reveal after enroll — the server never echoes it again.
	let newKey = $state<string | null>(null);

	function resetForm() {
		name = '';
		kind = 'kubernetes';
		accountKey = '';
	}

	function openEnroll() {
		resetForm();
		editing = null;
	}

	function openRename(d: UserDispatcher) {
		editing = d.id;
		name = d.name;
		kind = (d.kind as 'kubernetes' | 'docker' | 'http') ?? 'kubernetes';
	}

	function close() {
		editing = undefined;
	}

	async function save() {
		if (!name.trim()) {
			toasts.err(m.dispatch_err_name_required());
			return;
		}
		try {
			if (editing) {
				await actions.rename(editing, { name: name.trim() });
				toasts.ok(m.dispatch_toast_renamed());
				close();
			} else {
				const [account, provider] = accountKey ? accountKey.split('\0') : [];
				const r = await actions.enroll({
					name: name.trim(),
					kind,
					...(account ? { account, provider } : {}),
				});
				toasts.ok(m.dispatch_toast_enrolled());
				close();
				newKey = r.dispatcher_key;
			}
		} catch (e) {
			toasts.err((e as Error).message);
		}
	}

	function remove(d: UserDispatcher) {
		if (!confirm(m.dispatch_confirm_remove({ name: d.name }))) return;
		guard(actions.remove(d.id).then(() => toasts.ok(m.dispatch_toast_removed())));
	}

	async function copyKey() {
		if (!newKey) return;
		try {
			await navigator.clipboard.writeText(newKey);
			toasts.ok(m.dispatch_toast_key_copied());
		} catch {
			toasts.err(m.dispatch_err_copy_failed());
		}
	}

	const rows = $derived([...($dispatchers.data ?? [])]);
</script>

<div class="bar row">
	{#if heading}<Heading level={1}>{m.dispatch_heading()}</Heading>{/if}
	<div class="spacer"></div>
	<Button control variant="primary" onclick={openEnroll}>{m.dispatch_enroll_button()}</Button>
</div>

<div class="intro">
	<Text as="p" tone="muted" size="sm">
		{m.dispatch_intro_pre()}<Text variant="code">/dispatch</Text>{m.dispatch_intro_post()}
	</Text>
</div>

{#if $dispatchers.isLoading}
	<div class="empty"><span class="spin"></span></div>
{:else if rows.length === 0}
	<div class="empty"><Text tone="muted">{m.dispatch_empty()}</Text></div>
{:else}
	<div class="card table-card">
		<table class="disp">
			<thead>
				<tr>
					<th class="col-name">{m.dispatch_col_name()}</th>
					<th class="col-kind">{m.dispatch_col_type()}</th>
					<th class="col-status">{m.dispatch_col_status()}</th>
					<th class="col-key faint">{m.dispatch_col_key()}</th>
					<th class="col-seen">{m.dispatch_col_last_seen()}</th>
					<th class="col-actions">{m.dispatch_col_actions()}</th>
				</tr>
			</thead>
			<tbody>
				{#each rows as d (d.id)}
					<tr>
						<td class="col-name"><Text weight="semibold">{d.name}</Text></td>
						<td class="col-kind"><Badge>{d.kind}</Badge></td>
						<td class="col-status"><Badge tone={livenessTone(d)}>{livenessLabel(d)}</Badge></td>
						<td class="col-key faint truncate">{d.key_preview ?? '—'}</td>
						<td class="col-seen faint"><Timestamp value={d.last_seen_at} mode="relative" tone="inherit" /></td>
						<td class="col-actions">
							<div class="row acts">
								<Button onclick={() => openRename(d)}>{m.dispatch_rename()}</Button>
								<Button variant="danger" onclick={() => remove(d)}>{m.common_remove()}</Button>
							</div>
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
{/if}

{#if editing !== undefined}
	<Modal title={editing ? m.dispatch_modal_rename_title() : m.dispatch_modal_enroll_title()} onclose={close}>
		{#snippet body()}
			<div class="editor-body">
				<Field label={m.dispatch_field_name()}>
					<Input bind:value={name} placeholder="my-worker" />
				</Field>
				{#if !editing}
					<Field label={m.dispatch_field_type()}>
						<Select bind:value={kind}>
							<option value="kubernetes">kubernetes</option>
							<option value="docker">docker</option>
							<option value="http">http</option>
						</Select>
					</Field>
					<Field label={m.dispatch_field_default_account()}>
						<Select bind:value={accountKey}>
							<option value="">{m.dispatch_account_none_option()}</option>
							{#each $accounts.data ?? [] as a (a.id)}
								<option value={`${a.name}\0${primaryProvider(a)?.provider ?? ''}`}>{a.name} ({primaryProvider(a)?.provider ?? m.spawn_no_provider()})</option>
							{/each}
						</Select>
					</Field>
					<Text as="p" tone="muted" size="sm">
						{m.dispatch_enroll_hint()}
					</Text>
				{/if}
			</div>
		{/snippet}
		{#snippet footer()}
			<div class="spacer"></div>
			<Button onclick={close}>{m.common_cancel()}</Button>
			<Button variant="primary" onclick={save}>{editing ? m.common_save() : m.dispatch_enroll_action()}</Button>
		{/snippet}
	</Modal>
{/if}

{#if newKey}
	<Modal title={m.dispatch_modal_enrolled_title()} onclose={() => (newKey = null)}>
		{#snippet body()}
			<div class="editor-body">
				<Text as="p" tone="muted" size="sm">
					{m.dispatch_key_reveal_hint()}
				</Text>
				<div class="keybox"><code>{newKey}</code></div>
			</div>
		{/snippet}
		{#snippet footer()}
			<div class="spacer"></div>
			<Button onclick={copyKey}>{m.common_copy()}</Button>
			<Button variant="primary" onclick={() => (newKey = null)}>{m.dispatch_done()}</Button>
		{/snippet}
	</Modal>
{/if}

<style>
	.bar {
		margin-bottom: var(--sp-2);
	}
	.intro {
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
	.col-kind {
		width: 8rem;
	}
	.col-status {
		width: 8rem;
	}
	.col-seen {
		width: 9rem;
	}
	.col-actions {
		width: 13rem;
	}
	.acts {
		gap: var(--sp-1);
	}
	.editor-body {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.keybox {
		padding: var(--sp-2) var(--sp-3);
		background: var(--surface-2, var(--surface));
		border: 1px solid var(--border);
		border-radius: var(--radius-sm, 4px);
		overflow-x: auto;
	}
	.keybox code {
		font-size: var(--fs-sm);
		word-break: break-all;
	}
	@media (max-width: 720px) {
		.col-seen,
		.col-key {
			display: none;
		}
	}
</style>
