<!--
  GitHub connector setup (GH-CONN-1), extracted from GithubView so the `/github`
  view can host both the PR inbox (GH-UI-1) and connector settings side by side.

  The user configures GitHub accounts one at a time, pasting a credential
  (fine-grained PAT or GitHub App installation token) that the server encrypts at
  rest. The webui never sees the stored credential — only a masked preview.
-->
<script lang="ts">
	import {
		useGithubConnectors,
		useGithubConnectorActions,
		useMe,
		useUsers,
		type ConnectorInfo,
		type CreateConnector,
		type UpdateConnector
	} from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import {
		Button,
		Card,
		Cluster,
		Field,
		Heading,
		Input,
		Modal,
		Select,
		Stack,
		Text,
		Timestamp
	} from '@dorsk/tsumikit';

	type CredentialKind = CreateConnector['credential_kind'];

	const connectors = useGithubConnectors();
	const actions = useGithubConnectorActions();

	// Connectors are user-owned; the admin token has no user identity, so an
	// admin operator picks the owning user explicitly (mirrors the OAuth-account
	// vault, CCT-251).
	const me = useMe();
	const isAdmin = $derived($me.data?.role === 'admin');
	const users = useUsers(() => isAdmin);
	const activeUsers = $derived(($users.data ?? []).filter((u) => !u.revoked_at));

	let showModal = $state(false);
	// null = create mode; an id = editing that connector.
	let editingId = $state<string | null>(null);
	let name = $state('');
	let credentialKind = $state<CredentialKind>('pat');
	let credential = $state('');
	let reposText = $state('');
	let webhookSecret = $state('');
	let ownerId = $state('');
	let saving = $state(false);

	const editing = $derived(editingId !== null);

	$effect(() => {
		if (isAdmin && !ownerId && activeUsers.length > 0) ownerId = activeUsers[0].id;
	});

	function openCreate() {
		editingId = null;
		name = '';
		credentialKind = 'pat';
		credential = '';
		reposText = '';
		webhookSecret = '';
		showModal = true;
	}

	function openEdit(c: ConnectorInfo) {
		editingId = c.id;
		name = c.name;
		credentialKind = c.credential_kind;
		credential = ''; // blank = keep the stored credential
		reposText = c.repos.join(' ');
		webhookSecret = '';
		showModal = true;
	}

	function parseRepos(): string[] {
		return reposText
			.split(/[\s,]+/)
			.map((r) => r.trim())
			.filter(Boolean);
	}

	async function save() {
		if (!name.trim()) {
			toasts.err('Name is required');
			return;
		}
		if (!editing && !credential.trim()) {
			toasts.err('A credential is required for a new connector');
			return;
		}
		if (!editing && isAdmin && !ownerId) {
			toasts.err('Pick an owning user');
			return;
		}
		saving = true;
		try {
			if (editing && editingId) {
				const body: UpdateConnector = {
					name: name.trim(),
					repos: parseRepos(),
					// Blank credential/webhook = leave the stored ones unchanged.
					credential: credential.trim() || null,
					webhook_secret: webhookSecret.trim() || null
				};
				await actions.update(editingId, body);
				toasts.ok(`Connector ${name.trim()} updated`);
			} else {
				const body: CreateConnector = {
					name: name.trim(),
					credential_kind: credentialKind,
					credential: credential.trim(),
					repos: parseRepos(),
					webhook_secret: webhookSecret.trim() || null,
					user_id: isAdmin ? ownerId : null
				};
				await actions.create(body);
				toasts.ok(`Connector ${body.name} added`);
			}
			showModal = false;
		} catch (e) {
			toasts.err(e instanceof Error ? e.message : 'Failed to save connector');
		} finally {
			saving = false;
		}
	}

	let syncingId = $state<string | null>(null);

	async function refresh(id: string) {
		syncingId = id;
		try {
			const c = await actions.sync(id);
			if (c.last_error) toasts.err(`Poll failed: ${c.last_error}`);
			else toasts.ok('Refreshed from GitHub');
		} catch (e) {
			toasts.err(e instanceof Error ? e.message : 'Failed to refresh');
		} finally {
			syncingId = null;
		}
	}

	async function remove(id: string, label: string) {
		if (!confirm(`Remove the GitHub connector "${label}"? Its credential is deleted.`)) return;
		try {
			await actions.remove(id);
			toasts.ok('Connector removed');
		} catch (e) {
			toasts.err(e instanceof Error ? e.message : 'Failed to remove connector');
		}
	}

	const kindLabel = (k: string) => (k === 'app_installation' ? 'GitHub App' : 'Fine-grained PAT');
	const list = $derived($connectors.data ?? []);
</script>

<Stack gap="var(--sp-4)">
	<Cluster justify="space-between" align="center">
		<Heading level={2}>Connectors</Heading>
		<Button onclick={openCreate}>Add connector</Button>
	</Cluster>

	<Text as="p" tone="muted" size="sm">
		Configure GitHub accounts one at a time. The credential is encrypted on the server and never
		shown again — only a masked preview.
	</Text>

	{#if list.length === 0}
		<Card>
			<Text tone="muted">No connectors yet. Add one to enable pull-request review.</Text>
		</Card>
	{:else}
		<Stack gap="var(--sp-2)">
			{#each list as c (c.id)}
				<Card>
					<Cluster justify="space-between" align="center">
						<Stack gap="var(--sp-1)">
							<Cluster gap="var(--sp-2)" align="center">
								<Text weight="semibold">{c.name}</Text>
								<Text tone="muted" size="sm">{kindLabel(c.credential_kind)}</Text>
							</Cluster>
							<Text tone="muted" size="sm">
								Credential {c.credential_preview}
								{#if c.has_webhook_secret}· webhook secret set{/if}
							</Text>
							{#if c.repos.length > 0}
								<Text tone="muted" size="sm">Repos: {c.repos.join(', ')}</Text>
							{/if}
							<Text tone="muted" size="xs">
								Added <Timestamp value={c.created_at} mode="relative" />
								{#if c.last_polled_at}
									· polled <Timestamp value={c.last_polled_at} mode="relative" />
								{:else}
									· not polled yet
								{/if}
							</Text>
							{#if c.last_error}
								<Text tone="danger" size="sm">⚠ Last poll failed: {c.last_error}</Text>
							{/if}
						</Stack>
						<Cluster gap="var(--sp-2)" align="center">
							<Button
								size="sm"
								disabled={syncingId === c.id}
								onclick={() => refresh(c.id)}
							>
								{syncingId === c.id ? 'Refreshing…' : 'Refresh now'}
							</Button>
							<Button size="sm" onclick={() => openEdit(c)}>Edit</Button>
							<Button size="sm" variant="danger" onclick={() => remove(c.id, c.name)}>Remove</Button>
						</Cluster>
					</Cluster>
				</Card>
			{/each}
		</Stack>
	{/if}
</Stack>

{#if showModal}
	<Modal
		title={editing ? 'Edit GitHub connector' : 'Add GitHub connector'}
		onclose={() => (showModal = false)}
	>
		{#snippet body()}
			<Stack gap="var(--sp-3)">
				{#if isAdmin && !editing}
					<Field label="Owner">
						<Select bind:value={ownerId}>
							{#each activeUsers as u (u.id)}
								<option value={u.id}>{u.name}</option>
							{/each}
						</Select>
					</Field>
				{/if}
				<Field label="Name">
					<Input bind:value={name} placeholder="personal" />
				</Field>
				{#if !editing}
					<Field label="Credential type">
						<Select bind:value={credentialKind}>
							<option value="pat">Fine-grained PAT</option>
							<option value="app_installation">GitHub App installation token</option>
						</Select>
					</Field>
				{/if}
				<Field
					label={editing
						? 'New credential (leave blank to keep current)'
						: 'Credential (stored encrypted; never shown again)'}
				>
					<Input
						bind:value={credential}
						type="password"
						placeholder={editing ? 'leave blank to keep current' : 'github_pat_…'}
					/>
				</Field>
				{#if credentialKind === 'pat'}
					<Text as="p" tone="muted" size="xs">
						Create a <strong>fine-grained PAT</strong> at GitHub → Settings → Developer settings →
						Personal access tokens → Fine-grained tokens. Under <em>Repository access</em> select the
						repos you list below, then grant these <strong>repository permissions (read-only)</strong>:
						<strong>Pull requests</strong> and <strong>Contents</strong> (<strong>Metadata</strong> is
						required and granted automatically). No account/org permissions are needed.
						<strong>For private org repos</strong> the token must be approved by an org owner /
						SSO-authorized — and if the search keeps returning a 422 even then, use a
						<strong>classic PAT</strong> with <code>repo</code> scope: fine-grained tokens have
						limited support for the issue-search <code>@me</code>/user qualifiers. cctui polls every
						~5 min for PRs you authored or were asked to review; use “Refresh now” to poll on demand.
					</Text>
				{:else}
					<Text as="p" tone="muted" size="xs">
						Paste a GitHub App <strong>installation access token</strong> (the short-lived
						<code>ghs_…</code> token from <code>POST /app/installations/&#123;id&#125;/access_tokens</code>,
						not the app's private key). The installation must cover the repos you list below and grant
						read access to <strong>Pull requests</strong> and <strong>Contents</strong> (Metadata is
						implied). Note these tokens expire after ~1 hour — a PAT is simpler for a long-lived
						connector.
					</Text>
				{/if}
				<Field label="Repos (space/comma-separated owner/name, optional)">
					<Input bind:value={reposText} placeholder="dorskfr/kusaritoi dorskfr/cctui" />
				</Field>
				<Text as="p" tone="muted" size="xs">
					Each entry is an <strong>owner/name</strong> slug (e.g. <code>dorskfr/cctui</code>), separated
					by spaces or commas — not a username and not a URL. A bare <code>owner</code> tracks every
					repo that owner exposes to the token. Leave empty to track every repo the token can see.
				</Text>
				<Field label="Webhook secret (optional, stored encrypted)">
					<Input bind:value={webhookSecret} type="password" />
				</Field>
				<Cluster justify="flex-end" gap="var(--sp-2)">
					<Button size="sm" onclick={() => (showModal = false)}>Cancel</Button>
					<Button size="sm" variant="primary" disabled={saving} onclick={save}>
						{saving ? 'Saving…' : editing ? 'Save changes' : 'Add connector'}
					</Button>
				</Cluster>
			</Stack>
		{/snippet}
	</Modal>
{/if}
