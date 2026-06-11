<script lang="ts">
	import {
		useUserDispatchers,
		useDispatcherActions,
		type UserDispatcher,
		type UpsertDispatcher,
	} from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { dateOnly } from '$lib/format';

	const dispatchers = useUserDispatchers();
	const actions = useDispatcherActions();
	const guard = (p: Promise<unknown>) => p.catch((e: Error) => toasts.err(e.message));

	// Editor state. `editing` holds the id when updating, null when creating a
	// fresh one, undefined when the editor is closed.
	let editing = $state<string | null | undefined>(undefined);
	let name = $state('');
	let kind = $state<'http' | 'kubernetes'>('http');
	// http fields
	let url = $state('');
	let token = $state('');
	// kubernetes fields
	let namespace = $state('');
	let sourceCronjob = $state('');
	let cctuiUrl = $state('');

	function resetForm() {
		name = '';
		kind = 'http';
		url = '';
		token = '';
		namespace = '';
		sourceCronjob = '';
		cctuiUrl = '';
	}

	function openCreate() {
		resetForm();
		editing = null;
	}

	function openEdit(d: UserDispatcher) {
		resetForm();
		editing = d.id;
		name = d.name;
		kind = (d.kind as 'http' | 'kubernetes') ?? 'http';
		const c = d.config ?? {};
		url = (c.url as string) ?? '';
		// Never prefill the secret — the server only ever returns "<redacted>".
		token = '';
		namespace = (c.namespace as string) ?? '';
		sourceCronjob = (c.source_cronjob as string) ?? '';
		cctuiUrl = (c.cctui_url as string) ?? '';
	}

	function close() {
		editing = undefined;
	}

	function buildPayload(): UpsertDispatcher {
		const config: Record<string, unknown> =
			kind === 'http'
				? { url, ...(token ? { token } : {}) }
				: {
						namespace,
						source_cronjob: sourceCronjob,
						...(cctuiUrl ? { cctui_url: cctuiUrl } : {}),
					};
		return { name: name.trim(), kind, config };
	}

	async function save() {
		if (!name.trim()) {
			toasts.err('Name is required');
			return;
		}
		const body = buildPayload();
		try {
			if (editing) await actions.update(editing, body);
			else await actions.create(body);
			toasts.ok(editing ? 'Dispatcher updated' : 'Dispatcher created');
			close();
		} catch (e) {
			toasts.err((e as Error).message);
		}
	}

	function remove(d: UserDispatcher) {
		if (!confirm(`Delete dispatcher "${d.name}"?`)) return;
		guard(actions.remove(d.id).then(() => toasts.ok('Deleted')));
	}

	const rows = $derived([...($dispatchers.data ?? [])]);

	function summarize(d: UserDispatcher): string {
		const c = d.config ?? {};
		if (d.kind === 'http') {
			const tok = c.token ? ' · token set' : '';
			return `${(c.url as string) ?? ''}${tok}`;
		}
		return `${(c.namespace as string) ?? ''}/${(c.source_cronjob as string) ?? ''}`;
	}
</script>

<div class="bar row">
	<h1 class="page-title">Dispatchers</h1>
	<div class="spacer"></div>
	<button class="btn-control btn-primary" onclick={openCreate}>+ New dispatcher</button>
</div>

<p class="hint">
	Named targets for <code>/dispatch</code>. Reference one by its name; a name here
	overrides a global dispatcher of the same name, for you only.
</p>

{#if $dispatchers.isLoading}
	<div class="empty"><span class="spin"></span></div>
{:else if rows.length === 0}
	<div class="empty">No dispatchers yet.</div>
{:else}
	<div class="card table-card">
		<table class="disp">
			<thead>
				<tr>
					<th class="col-name">Name</th>
					<th class="col-kind">Type</th>
					<th class="col-config">Config</th>
					<th class="col-created">Created</th>
					<th class="col-actions">Actions</th>
				</tr>
			</thead>
			<tbody>
				{#each rows as d (d.id)}
					<tr>
						<td class="col-name"><span class="name">{d.name}</span></td>
						<td class="col-kind"><span class="badge">{d.kind}</span></td>
						<td class="col-config faint truncate">{summarize(d)}</td>
						<td class="col-created faint">{dateOnly(d.created_at)}</td>
						<td class="col-actions">
							<div class="row acts">
								<button class="btn btn-sm" onclick={() => openEdit(d)}>Edit</button>
								<button class="btn btn-sm btn-danger" onclick={() => remove(d)}>Delete</button>
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
			<h2>{editing ? 'Edit dispatcher' : 'New dispatcher'}</h2>
			<label class="fld">
				<span>Name</span>
				<input class="input" bind:value={name} placeholder="my-worker" />
			</label>
			<label class="fld">
				<span>Type</span>
				<select class="input" bind:value={kind}>
					<option value="http">http</option>
					<option value="kubernetes">kubernetes</option>
				</select>
			</label>
			{#if kind === 'http'}
				<label class="fld">
					<span>URL</span>
					<input class="input" bind:value={url} placeholder="https://dispatcher.example/dispatch" />
				</label>
				<label class="fld">
					<span>Bearer token</span>
					<input
						class="input"
						type="password"
						bind:value={token}
						placeholder={editing ? 'leave blank to keep current' : 'optional'}
					/>
				</label>
			{:else}
				<label class="fld">
					<span>Namespace</span>
					<input class="input" bind:value={namespace} placeholder="workers" />
				</label>
				<label class="fld">
					<span>Source CronJob</span>
					<input class="input" bind:value={sourceCronjob} placeholder="worker-template" />
				</label>
				<label class="fld">
					<span>CCTUI URL (optional)</span>
					<input class="input" bind:value={cctuiUrl} placeholder="https://cctui.example.com" />
				</label>
			{/if}
			<div class="row editor-acts">
				<div class="spacer"></div>
				<button class="btn btn-sm" onclick={close}>Cancel</button>
				<button class="btn btn-sm btn-primary" onclick={save}>Save</button>
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
	.col-kind {
		width: 8rem;
	}
	.col-created {
		width: 8rem;
	}
	.col-actions {
		width: 12rem;
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
	@media (max-width: 720px) {
		.col-created {
			display: none;
		}
	}
</style>
