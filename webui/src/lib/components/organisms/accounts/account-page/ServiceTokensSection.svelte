<script lang="ts">
	import type { ServiceTokenRow } from '@bindings/ServiceTokenRow';
	import {
		Button,
		Card,
		type Column,
		ConfirmModal,
		DataTable,
		IconButton,
		Select,
		Text,
		Timestamp
	} from '@dorsk/tsumikit';
	import EditEntityModal from '$lib/components/molecules/EditEntityModal.svelte';
	import SecretReveal from '$lib/components/molecules/SecretReveal.svelte';
	import { useServiceTokens, useServiceTokenActions, type OAuthAccount } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { errMessage } from '$lib/api';
	import { providerLabel } from '$lib/providers';
	import { m } from '$lib/paraglide/messages';

	let { account }: { account: OAuthAccount } = $props();

	const tokens = useServiceTokens(() => account.id);
	const actions = useServiceTokenActions();
	const guard = (p: Promise<unknown>) => p.catch((e: unknown) => toasts.error(errMessage(e)));

	const rows = $derived(tokens.data ?? []);
	const providers = $derived(account.providers);
	const providerOptions = $derived(
		providers.map((p) => ({ value: p.id, label: providerLabel(p.provider) }))
	);
	let providerId = $state('');
	const selectedProvider = $derived(
		providers.find((p) => p.id === providerId)?.id ?? providers[0]?.id ?? ''
	);

	let mintOpen = $state(false);
	let secret = $state<{ title: string; value: string } | null>(null);
	let dropTarget = $state<ServiceTokenRow | null>(null);

	const columns: Column<ServiceTokenRow>[] = [
		{ key: 'label', label: m.service_tokens_col_label(), role: 'title' },
		{ key: 'provider', label: m.service_tokens_col_provider(), width: '9rem', nowrap: true, role: 'meta' },
		{ key: 'created', label: m.service_tokens_col_created(), width: '11rem', nowrap: true, role: 'meta' },
		{ key: 'used', label: m.service_tokens_col_last_used(), width: '11rem', nowrap: true, role: 'meta' },
		{ key: 'actions', label: '', width: '4rem', nowrap: true, align: 'right', role: 'actions' }
	];

	function mint(label: string | null) {
		if (!label) {
			toasts.error(m.service_tokens_label_required());
			return;
		}
		if (!selectedProvider) return;
		guard(
			actions.mint(account.id, { label, provider_id: selectedProvider }).then((r) => {
				secret = { title: m.service_tokens_secret_title({ label: r.row.label }), value: r.token };
			})
		);
	}

	function revoke(t: ServiceTokenRow) {
		dropTarget = null;
		guard(actions.revoke(account.id, t.id).then(() => toasts.ok(m.service_tokens_revoked())));
	}
</script>

{#snippet colProvider(t: ServiceTokenRow)}
	<Text size="xs" tone="muted">{providerLabel(t.provider)}</Text>
{/snippet}
{#snippet colCreated(t: ServiceTokenRow)}
	<Timestamp value={t.created_at} mode="short-iso" mono size="xs" tone="faint" />
{/snippet}
{#snippet colUsed(t: ServiceTokenRow)}
	{#if t.last_used_at}
		<Timestamp value={t.last_used_at} mode="relative" mono size="xs" tone="faint" />
	{:else}
		<Text size="xs" tone="faint">{m.service_tokens_never_used()}</Text>
	{/if}
{/snippet}
{#snippet colActions(t: ServiceTokenRow)}
	<IconButton
		inline
		hoverDanger
		icon="trash"
		size={14}
		label={m.service_tokens_revoke()}
		title={m.service_tokens_revoke()}
		onclick={() => (dropTarget = t)}
	/>
{/snippet}

<Card title={m.service_tokens_title()} subtitle={m.service_tokens_help()}>
	{#snippet actions()}
		{#if providers.length > 1}
			<Select
				size="sm"
				width="auto"
				bind:value={providerId}
				options={providerOptions}
				aria-label={m.service_tokens_col_provider()}
			/>
		{/if}
		<Button
			size="sm"
			variant="primary"
			disabled={!selectedProvider}
			onclick={() => (mintOpen = true)}
		>
			{m.service_tokens_new()}
		</Button>
	{/snippet}

	<DataTable
		{columns}
		{rows}
		rowKey={(t) => t.id}
		responsive="stack"
		loading={tokens.isLoading}
		loadingLabel={m.common_loading()}
		empty={m.service_tokens_empty()}
		layout="fixed"
		cellSnippets={{ provider: colProvider, created: colCreated, used: colUsed, actions: colActions }}
	/>
</Card>

{#if mintOpen}
	<EditEntityModal
		title={m.service_tokens_new_title()}
		fieldLabel={m.service_tokens_field_label()}
		placeholder={m.service_tokens_label_placeholder()}
		hint={m.service_tokens_label_hint()}
		saveLabel={m.service_tokens_create()}
		onsave={(label) => mint(label)}
		onclose={() => (mintOpen = false)}
	/>
{/if}

{#if secret}
	<SecretReveal title={secret.title} secret={secret.value} onclose={() => (secret = null)} />
{/if}

{#if dropTarget}
	{@const t = dropTarget}
	<ConfirmModal
		open
		tone="danger"
		title={m.service_tokens_revoke()}
		message={m.service_tokens_confirm_revoke({ label: t.label })}
		confirmLabel={m.service_tokens_revoke()}
		onconfirm={() => revoke(t)}
		oncancel={() => (dropTarget = null)}
	/>
{/if}
