<script lang="ts">
	import {
		useUserDispatchers,
		useDispatcherActions,
		type UserDispatcher,
		type UpsertDispatcher,
	} from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { dateOnly } from '$lib/format';
	import { Badge, Button, Field, Heading, Input, Modal, Select, Text } from '@dorsk/tsumikit';
	import { summarize } from './dispatchers.logic';

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
</script>

<div class="bar row">
	<Heading level={1}>Dispatchers</Heading>
	<div class="spacer"></div>
	<Button control variant="primary" onclick={openCreate}>+ New dispatcher</Button>
</div>

<div class="intro">
	<Text as="p" tone="muted" size="sm">
		Named targets for <Text variant="code">/dispatch</Text>. Reference one by its name; a name
		here overrides a global dispatcher of the same name, for you only.
	</Text>
</div>

{#if $dispatchers.isLoading}
	<div class="empty"><span class="spin"></span></div>
{:else if rows.length === 0}
	<div class="empty"><Text tone="muted">No dispatchers yet.</Text></div>
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
						<td class="col-name"><Text weight="semibold">{d.name}</Text></td>
						<td class="col-kind"><Badge>{d.kind}</Badge></td>
						<td class="col-config faint truncate">{summarize(d)}</td>
						<td class="col-created faint">{dateOnly(d.created_at)}</td>
						<td class="col-actions">
							<div class="row acts">
								<Button size="sm" onclick={() => openEdit(d)}>Edit</Button>
								<Button size="sm" variant="danger" onclick={() => remove(d)}>Delete</Button>
							</div>
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
{/if}

{#if editing !== undefined}
	<Modal title={editing ? 'Edit dispatcher' : 'New dispatcher'} onclose={close}>
		{#snippet body()}
			<div class="editor-body">
				<Field label="Name">
					<Input bind:value={name} placeholder="my-worker" />
				</Field>
				<Field label="Type">
					<Select bind:value={kind}>
						<option value="http">http</option>
						<option value="kubernetes">kubernetes</option>
					</Select>
				</Field>
				{#if kind === 'http'}
					<Field label="URL">
						<Input bind:value={url} placeholder="https://dispatcher.example/dispatch" />
					</Field>
					<Field label="Bearer token">
						<Input
							type="password"
							bind:value={token}
							placeholder={editing ? 'leave blank to keep current' : 'optional'}
						/>
					</Field>
				{:else}
					<Field label="Namespace">
						<Input bind:value={namespace} placeholder="workers" />
					</Field>
					<Field label="Source CronJob">
						<Input bind:value={sourceCronjob} placeholder="worker-template" />
					</Field>
					<Field label="CCTUI URL (optional)">
						<Input bind:value={cctuiUrl} placeholder="https://cctui.example.com" />
					</Field>
				{/if}
			</div>
		{/snippet}
		{#snippet footer()}
			<div class="spacer"></div>
			<Button size="sm" onclick={close}>Cancel</Button>
			<Button size="sm" variant="primary" onclick={save}>Save</Button>
		{/snippet}
	</Modal>
{/if}

<style>
	.bar {
		margin-bottom: var(--sp-2);
	}
	/* Typography from the Text atom; only the page rhythm lives here. */
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
	.col-created {
		width: 8rem;
	}
	.col-actions {
		width: 12rem;
	}
	.acts {
		gap: var(--sp-1);
	}
	.editor-body {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	@media (max-width: 720px) {
		.col-created {
			display: none;
		}
	}
</style>
