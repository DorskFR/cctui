<!--
  GitHub connector setup (GH-CONN-1), extracted from GithubView so the `/github`
  view can host both the PR inbox (GH-UI-1) and connector settings side by side.

  The user configures GitHub accounts one at a time, pasting a credential
  (classic PAT, fine-grained PAT, or GitHub App installation token) that the
  server encrypts at rest. The webui never sees the stored credential — only a
  masked preview.
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
	import { ghreviewUrl } from '$lib/config';
	import { deprovisionGhreviewAccount, provisionGhreviewAccount } from '$lib/ghreview';
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
	import { m } from '$lib/paraglide/messages';

	type CredentialKind = CreateConnector['credential_kind'];

	const connectors = useGithubConnectors();
	const actions = useGithubConnectorActions();

	// Connectors are user-owned; the admin token has no user identity, so an
	// admin operator picks the owning user explicitly (mirrors the
	// OAuth-account vault).
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
			toasts.err(m.github_name_required());
			return;
		}
		if (!editing && !credential.trim()) {
			toasts.err(m.github_credential_required());
			return;
		}
		if (!editing && isAdmin && !ownerId) {
			toasts.err(m.github_pick_owner());
			return;
		}
		saving = true;
		try {
			const pat = credential.trim();
			let connectorId: string;
			if (editing && editingId) {
				const body: UpdateConnector = {
					name: name.trim(),
					repos: parseRepos(),
					// Blank credential/webhook = leave the stored ones unchanged.
					credential: pat || null,
					webhook_secret: webhookSecret.trim() || null
				};
				const updated = await actions.update(editingId, body);
				connectorId = updated.id;
				toasts.ok(m.github_connector_updated({ name: name.trim() }));
			} else {
				const body: CreateConnector = {
					name: name.trim(),
					credential_kind: credentialKind,
					credential: pat,
					repos: parseRepos(),
					webhook_secret: webhookSecret.trim() || null,
					user_id: isAdmin ? ownerId : null
				};
				const created = await actions.create(body);
				connectorId = created.id;
				toasts.ok(m.github_connector_added({ name: body.name }));
			}
			showModal = false;
			if (credentialKind === 'pat' && pat && ghreviewUrl()) {
				try {
					const login = await provisionGhreviewAccount(connectorId, pat);
					if (login) toasts.ok(m.github_review_provisioned({ login }));
				} catch (e) {
					toasts.err(
						m.github_review_provision_failed({ error: e instanceof Error ? e.message : String(e) })
					);
				}
			}
		} catch (e) {
			toasts.err(e instanceof Error ? e.message : m.github_save_failed());
		} finally {
			saving = false;
		}
	}

	let syncingId = $state<string | null>(null);

	async function refresh(id: string) {
		syncingId = id;
		try {
			const c = await actions.sync(id);
			if (c.last_error) toasts.err(m.github_poll_failed({ error: c.last_error }));
			else toasts.ok(m.github_refreshed());
		} catch (e) {
			toasts.err(e instanceof Error ? e.message : m.github_refresh_failed());
		} finally {
			syncingId = null;
		}
	}

	async function remove(id: string, label: string) {
		if (!confirm(m.github_remove_confirm({ label }))) return;
		try {
			await actions.remove(id);
			toasts.ok(m.github_connector_removed());
			if (ghreviewUrl()) {
				try {
					await deprovisionGhreviewAccount(id);
				} catch (e) {
					toasts.err(
						m.github_review_deprovision_failed({ error: e instanceof Error ? e.message : String(e) })
					);
				}
			}
		} catch (e) {
			toasts.err(e instanceof Error ? e.message : m.github_remove_failed());
		}
	}

	const kindLabel = (k: string) => (k === 'app_installation' ? m.github_kind_app() : m.github_kind_pat());
	const list = $derived($connectors.data ?? []);
</script>

<Stack gap="var(--sp-4)">
	<Cluster justify="space-between" align="center">
		<Heading level={2}>{m.github_connectors_heading()}</Heading>
		<Button onclick={openCreate}>{m.github_add_connector()}</Button>
	</Cluster>

	<Text as="p" tone="muted" size="sm">
		{m.github_connectors_intro()}
	</Text>

	{#if list.length === 0}
		<Card>
			<Text tone="muted">{m.github_no_connectors()}</Text>
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
								{m.github_credential_preview({ preview: c.credential_preview })}
								{#if c.has_webhook_secret}{m.github_webhook_secret_set()}{/if}
							</Text>
							{#if c.repos.length > 0}
								<Text tone="muted" size="sm">{m.github_repos_label({ repos: c.repos.join(', ') })}</Text>
							{/if}
							<Text tone="muted" size="xs">
								{m.github_added_label()} <Timestamp value={c.created_at} mode="relative" />
								{#if c.last_polled_at}
									{m.github_polled_label()} <Timestamp value={c.last_polled_at} mode="relative" />
								{:else}
									{m.github_not_polled()}
								{/if}
							</Text>
							{#if c.last_error}
								<Text tone="danger" size="sm">{m.github_last_poll_failed({ error: c.last_error })}</Text>
							{/if}
						</Stack>
						<Cluster gap="var(--sp-2)" align="center">
							<Button
								disabled={syncingId === c.id}
								onclick={() => refresh(c.id)}
							>
								{syncingId === c.id ? m.github_refreshing() : m.github_refresh_now()}
							</Button>
							<Button onclick={() => openEdit(c)}>{m.common_edit()}</Button>
							<Button variant="danger" onclick={() => remove(c.id, c.name)}>{m.common_remove()}</Button>
						</Cluster>
					</Cluster>
				</Card>
			{/each}
		</Stack>
	{/if}
</Stack>

{#if showModal}
	<Modal
		title={editing ? m.github_edit_connector_title() : m.github_add_connector_title()}
		onclose={() => (showModal = false)}
	>
		{#snippet body()}
			<Stack gap="var(--sp-3)">
				{#if isAdmin && !editing}
					<Field label={m.github_field_owner()}>
						<Select bind:value={ownerId}>
							{#each activeUsers as u (u.id)}
								<option value={u.id}>{u.name}</option>
							{/each}
						</Select>
					</Field>
				{/if}
				<Field label={m.github_field_name()}>
					<Input bind:value={name} placeholder={m.github_name_placeholder()} />
				</Field>
				{#if !editing}
					<Field label={m.github_field_credential_type()}>
						<Select bind:value={credentialKind}>
							<option value="pat">{m.github_kind_pat()}</option>
							<option value="app_installation">{m.github_credential_app_option()}</option>
						</Select>
					</Field>
				{/if}
				<Field
					label={editing
						? m.github_field_new_credential()
						: m.github_field_credential()}
				>
					<Input
						bind:value={credential}
						type="password"
						placeholder={editing ? m.github_credential_placeholder_keep() : 'ghp_… or github_pat_…'}
					/>
				</Field>
				{#if credentialKind === 'pat'}
					<Text as="p" tone="muted" size="xs">
						Both token flavors work (GitHub → Settings → Developer settings → Personal access
						tokens). A <strong>classic PAT</strong> (<code>ghp_…</code>) with <code>repo</code> scope is
						the simplest: it can track your <strong>whole account</strong> with no repos listed, since
						only classic tokens can run the cross-repo issue search. A <strong>fine-grained PAT</strong>
						(<code>github_pat_…</code>) works only when you <strong>list explicit repos below</strong>
						(cctui lists each repo's PRs directly): under <em>Repository access</em> select those repos,
						then grant <strong>read-only repository permissions</strong> <strong>Pull requests</strong>
						and <strong>Contents</strong> (<strong>Metadata</strong> is granted automatically); for
						private org repos the token must be approved by an org owner / SSO-authorized. cctui polls
						every ~5 min for PRs you authored or were asked to review; use “Refresh now” to poll on
						demand.
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
				<Field label={m.github_field_repos()}>
					<Input bind:value={reposText} placeholder="dorskfr/kusaritoi dorskfr/cctui" />
				</Field>
				<Text as="p" tone="muted" size="xs">
					Each entry is an <strong>owner/name</strong> slug (e.g. <code>dorskfr/cctui</code>), separated
					by spaces or commas — not a username and not a URL. A bare <code>owner</code> tracks every
					repo that owner exposes to the token. Leave empty to track every repo the token can see.
				</Text>
				<Field label={m.github_field_webhook_secret()}>
					<Input bind:value={webhookSecret} type="password" />
				</Field>
				<Cluster justify="flex-end" gap="var(--sp-2)">
					<Button onclick={() => (showModal = false)}>{m.common_cancel()}</Button>
					<Button variant="primary" disabled={saving} onclick={save}>
						{saving ? m.github_saving() : editing ? m.github_save_changes() : m.github_add_connector()}
					</Button>
				</Cluster>
			</Stack>
		{/snippet}
	</Modal>
{/if}
