<!--
  GitHub account setup for the review center. Credentials live in the ghreview
  backend; this pastes a PAT to POST /v1/accounts and lists/removes the caller's
  accounts. ghreviewUrl unset means the review backend isn't deployed.
-->
<script lang="ts">
	import { ghreviewUrl } from '$lib/config';
	import {
		addGhreviewAccount,
		listGhreviewAccounts,
		removeGhreviewAccount,
		type GhreviewAccount
	} from '$lib/ghreview';
	import { toasts } from '$lib/toast.svelte';
	import {
		Button,
		Card,
		Cluster,
		Field,
		Heading,
		Input,
		Modal,
		Stack,
		Text,
		Timestamp
	} from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	const configured = $derived(ghreviewUrl() !== null);

	let accounts = $state<GhreviewAccount[]>([]);
	let loading = $state(false);

	async function load() {
		if (!configured) return;
		loading = true;
		try {
			accounts = await listGhreviewAccounts();
		} catch (e) {
			toasts.err(e instanceof Error ? e.message : m.github_save_failed());
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		void load();
	});

	let showModal = $state(false);
	let pat = $state('');
	let login = $state('');
	let saving = $state(false);

	function openAdd() {
		pat = '';
		login = '';
		showModal = true;
	}

	async function save() {
		if (!pat.trim()) {
			toasts.err(m.github_credential_required());
			return;
		}
		saving = true;
		try {
			const account = await addGhreviewAccount(pat.trim(), login.trim() || undefined);
			toasts.ok(m.github_connector_added({ name: account.login }));
			showModal = false;
			await load();
		} catch (e) {
			toasts.err(e instanceof Error ? e.message : m.github_save_failed());
		} finally {
			saving = false;
		}
	}

	async function remove(account: GhreviewAccount) {
		if (!confirm(m.github_remove_confirm({ label: account.login }))) return;
		try {
			await removeGhreviewAccount(account.id);
			toasts.ok(m.github_connector_removed());
			await load();
		} catch (e) {
			toasts.err(e instanceof Error ? e.message : m.github_remove_failed());
		}
	}
</script>

<Stack gap="var(--sp-4)">
	<Cluster justify="space-between" align="center">
		<Heading level={2}>{m.github_connectors_heading()}</Heading>
		{#if configured}
			<Button onclick={openAdd}>{m.github_add_connector()}</Button>
		{/if}
	</Cluster>

	<Text as="p" tone="muted" size="sm">{m.github_connectors_intro()}</Text>

	{#if !configured}
		<Card>
			<Text tone="muted">{m.github_no_review_accounts_body()}</Text>
		</Card>
	{:else if loading && accounts.length === 0}
		<Text tone="muted">{m.common_loading()}</Text>
	{:else if accounts.length === 0}
		<Card>
			<Text tone="muted">{m.github_no_connectors()}</Text>
		</Card>
	{:else}
		<Stack gap="var(--sp-2)">
			{#each accounts as a (a.id)}
				<Card>
					<Cluster justify="space-between" align="center">
						<Stack gap="var(--sp-1)">
							<Text weight="semibold">{a.login}</Text>
							{#if a.created_at}
								<Text tone="muted" size="xs">
									{m.github_added_label()} <Timestamp value={a.created_at} mode="relative" />
								</Text>
							{/if}
						</Stack>
						<Button variant="danger" onclick={() => remove(a)}>{m.common_remove()}</Button>
					</Cluster>
				</Card>
			{/each}
		</Stack>
	{/if}
</Stack>

{#if showModal}
	<Modal title={m.github_add_connector_title()} onclose={() => (showModal = false)}>
		{#snippet body()}
			<Stack gap="var(--sp-3)">
				<Field label={m.github_field_credential()}>
					<Input bind:value={pat} type="password" placeholder="ghp_… or github_pat_…" />
				</Field>
				<Field label={m.github_account_label()}>
					<Input bind:value={login} placeholder="octocat" />
				</Field>
				<Cluster justify="flex-end" gap="var(--sp-2)">
					<Button onclick={() => (showModal = false)}>{m.common_cancel()}</Button>
					<Button variant="primary" disabled={saving} onclick={save}>
						{saving ? m.github_saving() : m.github_add_connector()}
					</Button>
				</Cluster>
			</Stack>
		{/snippet}
	</Modal>
{/if}
