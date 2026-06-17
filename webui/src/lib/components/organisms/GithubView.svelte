<!--
  The GitHub integration view (CCT-375 / GH-CAP-1, connector setup GH-CONN-1).

  Lazy-loaded payload behind the `/github` route. GH-CONN-1 adds the per-user
  connector setup UI: the user configures GitHub accounts one at a time, pasting
  a credential (fine-grained PAT or GitHub App installation token) that the
  server encrypts at rest. The webui never sees the stored credential — only a
  masked preview is shown back. PR review and diffs land in later stories.
-->
<script lang="ts">
	import {
		useGithubConnectors,
		useGithubConnectorActions,
		useMe,
		useUsers,
		type CreateConnector
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
	let name = $state('');
	let credentialKind = $state<CredentialKind>('pat');
	let credential = $state('');
	let reposText = $state('');
	let webhookSecret = $state('');
	let ownerId = $state('');
	let saving = $state(false);

	$effect(() => {
		if (isAdmin && !ownerId && activeUsers.length > 0) ownerId = activeUsers[0].id;
	});

	function openCreate() {
		name = '';
		credentialKind = 'pat';
		credential = '';
		reposText = '';
		webhookSecret = '';
		showModal = true;
	}

	async function create() {
		if (!name.trim() || !credential.trim()) {
			toasts.err('Name and credential are required');
			return;
		}
		if (isAdmin && !ownerId) {
			toasts.err('Pick an owning user');
			return;
		}
		saving = true;
		try {
			const repos = reposText
				.split(/[\s,]+/)
				.map((r) => r.trim())
				.filter(Boolean);
			const body: CreateConnector = {
				name: name.trim(),
				credential_kind: credentialKind,
				credential: credential.trim(),
				repos,
				webhook_secret: webhookSecret.trim() || null,
				user_id: isAdmin ? ownerId : null
			};
			await actions.create(body);
			toasts.ok(`Connector ${body.name} added`);
			showModal = false;
		} catch (e) {
			toasts.err(e instanceof Error ? e.message : 'Failed to add connector');
		} finally {
			saving = false;
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
		<Heading level={1}>GitHub</Heading>
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
							</Text>
						</Stack>
						<Button size="sm" variant="danger" onclick={() => remove(c.id, c.name)}>Remove</Button>
					</Cluster>
				</Card>
			{/each}
		</Stack>
	{/if}
</Stack>

{#if showModal}
	<Modal title="Add GitHub connector" onclose={() => (showModal = false)}>
		{#snippet body()}
			<Stack gap="var(--sp-3)">
				{#if isAdmin}
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
				<Field label="Credential type">
					<Select bind:value={credentialKind}>
						<option value="pat">Fine-grained PAT</option>
						<option value="app_installation">GitHub App installation token</option>
					</Select>
				</Field>
				<Field label="Credential (stored encrypted; never shown again)">
					<Input bind:value={credential} type="password" placeholder="github_pat_…" />
				</Field>
				<Field label="Repos (space/comma-separated owner/name, optional)">
					<Input bind:value={reposText} placeholder="acme/api acme/web" />
				</Field>
				<Field label="Webhook secret (optional, stored encrypted)">
					<Input bind:value={webhookSecret} type="password" />
				</Field>
				<Cluster justify="flex-end" gap="var(--sp-2)">
					<Button size="sm" onclick={() => (showModal = false)}>Cancel</Button>
					<Button size="sm" variant="primary" disabled={saving} onclick={create}>
						{saving ? 'Saving…' : 'Add connector'}
					</Button>
				</Cluster>
			</Stack>
		{/snippet}
	</Modal>
{/if}
