<!--
  Dispatcher management, extracted from the /dispatchers route (CCT-403) so it
  can be hosted under the Accounts page (the single home for everything that
  connects to something external) as well as its own route.
-->
<script lang="ts">
	import {
		useUserDispatchers,
		useDispatcherActions,
		type UserDispatcher,
	} from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { Badge, Button, Field, Heading, Input, Modal, Select, Text, Timestamp } from '@dorsk/tsumikit';
	import { livenessLabel, livenessTone } from '$lib/dispatchers.logic';

	// When embedded under Accounts the page already shows an <h1>, so the panel's
	// own heading drops a level; standalone it stays an <h1>.
	let { heading = true }: { heading?: boolean } = $props();

	const dispatchers = useUserDispatchers();
	const actions = useDispatcherActions();
	const guard = (p: Promise<unknown>) => p.catch((e: Error) => toasts.err(e.message));

	// Enroll editor state. `editing` holds the id when renaming, null when
	// enrolling fresh, undefined when the editor is closed.
	let editing = $state<string | null | undefined>(undefined);
	let name = $state('');
	let kind = $state<'kubernetes' | 'docker' | 'http'>('kubernetes');

	// One-shot key reveal after enroll — the server never echoes it again.
	let newKey = $state<string | null>(null);

	function resetForm() {
		name = '';
		kind = 'kubernetes';
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
			toasts.err('Name is required');
			return;
		}
		try {
			if (editing) {
				await actions.rename(editing, { name: name.trim() });
				toasts.ok('Dispatcher renamed');
				close();
			} else {
				const r = await actions.enroll({ name: name.trim(), kind });
				toasts.ok('Dispatcher enrolled');
				close();
				newKey = r.dispatcher_key;
			}
		} catch (e) {
			toasts.err((e as Error).message);
		}
	}

	function remove(d: UserDispatcher) {
		if (!confirm(`Remove dispatcher "${d.name}"?`)) return;
		guard(actions.remove(d.id).then(() => toasts.ok('Removed')));
	}

	async function copyKey() {
		if (!newKey) return;
		try {
			await navigator.clipboard.writeText(newKey);
			toasts.ok('Key copied');
		} catch {
			toasts.err('Copy failed — select and copy manually');
		}
	}

	const rows = $derived([...($dispatchers.data ?? [])]);
</script>

<div class="bar row">
	{#if heading}<Heading level={1}>Dispatchers</Heading>{/if}
	<div class="spacer"></div>
	<Button control variant="primary" onclick={openEnroll}>+ Enroll dispatcher</Button>
</div>

<div class="intro">
	<Text as="p" tone="muted" size="sm">
		Standalone executor services enrolled to your account. Each dials out to the server over a
		WebSocket and runs dispatched workers on its host. Reference one by its name in
		<Text variant="code">/dispatch</Text>; a name here overrides a global dispatcher of the same
		name, for you only.
	</Text>
</div>

{#if $dispatchers.isLoading}
	<div class="empty"><span class="spin"></span></div>
{:else if rows.length === 0}
	<div class="empty"><Text tone="muted">No dispatchers enrolled yet.</Text></div>
{:else}
	<div class="card table-card">
		<table class="disp">
			<thead>
				<tr>
					<th class="col-name">Name</th>
					<th class="col-kind">Type</th>
					<th class="col-status">Status</th>
					<th class="col-key faint">Key</th>
					<th class="col-seen">Last seen</th>
					<th class="col-actions">Actions</th>
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
								<Button size="sm" onclick={() => openRename(d)}>Rename</Button>
								<Button size="sm" variant="danger" onclick={() => remove(d)}>Remove</Button>
							</div>
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
{/if}

{#if editing !== undefined}
	<Modal title={editing ? 'Rename dispatcher' : 'Enroll dispatcher'} onclose={close}>
		{#snippet body()}
			<div class="editor-body">
				<Field label="Name">
					<Input bind:value={name} placeholder="my-worker" />
				</Field>
				{#if !editing}
					<Field label="Type">
						<Select bind:value={kind}>
							<option value="kubernetes">kubernetes</option>
							<option value="docker">docker</option>
							<option value="http">http</option>
						</Select>
					</Field>
					<Text as="p" tone="muted" size="sm">
						An enrollment key is generated and shown once. Configure your dispatcher binary with it
						so it can dial out.
					</Text>
				{/if}
			</div>
		{/snippet}
		{#snippet footer()}
			<div class="spacer"></div>
			<Button size="sm" onclick={close}>Cancel</Button>
			<Button size="sm" variant="primary" onclick={save}>{editing ? 'Save' : 'Enroll'}</Button>
		{/snippet}
	</Modal>
{/if}

{#if newKey}
	<Modal title="Dispatcher enrolled" onclose={() => (newKey = null)}>
		{#snippet body()}
			<div class="editor-body">
				<Text as="p" tone="muted" size="sm">
					Copy this enrollment key now — it is shown only once and cannot be retrieved later.
					Configure your dispatcher binary with it.
				</Text>
				<div class="keybox"><code>{newKey}</code></div>
			</div>
		{/snippet}
		{#snippet footer()}
			<div class="spacer"></div>
			<Button size="sm" onclick={copyKey}>Copy</Button>
			<Button size="sm" variant="primary" onclick={() => (newKey = null)}>Done</Button>
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
