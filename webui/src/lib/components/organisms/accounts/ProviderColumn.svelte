<script lang="ts">
	import type { AccountProvider } from '$lib/queries';
	import { useAccountActions } from '$lib/queries';
	import { compact } from '$lib/format';
	import UsageBars from '$lib/components/molecules/UsageBars.svelte';
	import AdapterIcon from '$lib/components/atoms/AdapterIcon.svelte';
	import { Button, IconButton, Switch, Text, Timestamp } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { providerLabel } from '$lib/providers';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { toasts } from '$lib/toast.svelte';
	import { errMessage } from '$lib/api';

	let {
		provider,
		usageEnabled = true,
		canManage = false,
		canRemove = false,
		onedit,
		onreauth,
		onremove
	}: {
		provider: AccountProvider;
		usageEnabled?: boolean;
		canManage?: boolean;
		canRemove?: boolean;
		onedit?: () => void;
		onreauth?: () => void;
		onremove?: () => void;
	} = $props();

	const p = $derived(provider);
	const native = $derived(p.provider === 'anthropic' || p.provider === 'openai');
	const summary = $derived.by(() => {
		const aliases = Object.keys(p.model_aliases ?? {});
		if (aliases.length) return aliases.join(', ');
		return (p.models ?? []).map((mo) => mo.label || mo.model).join(', ');
	});

	const actions = useAccountActions();
	const qc = useQueryClient();
	let pinned = $state<boolean | null>(null);
	const headerPin = $derived(pinned ?? p.header_pin);
	const canPin = $derived(canManage && (native || p.provider === 'fireworks') && !p.managed);
	$effect(() => {
		if (pinned !== null && p.header_pin === pinned) pinned = null;
	});
	async function togglePin() {
		const next = !headerPin;
		pinned = next;
		try {
			await actions.updateProvider(p.account_id, p.id, { header_pin: next });
			qc.invalidateQueries({ queryKey: ['accounts-usage'] });
		} catch (e) {
			pinned = !next;
			toasts.error(errMessage(e));
		}
	}
</script>

<div class="column">
	<div class="head">
		<span class="mark" title={providerLabel(p.provider)}>
			<AdapterIcon provider={p.provider} size={18} />
		</span>
		<Text as="span" size="sm" weight="semibold">{providerLabel(p.provider)}</Text>
		{#if p.managed}
			<Text as="span" tone="faint" size="xs">{m.providers_managed()}</Text>
		{:else if summary}
			<span class="summary" title={summary}><Text as="span" tone="faint" size="xs">{summary}</Text></span>
		{/if}
		<span class="spacer"></span>
		{#if canManage}
			{#if p.needs_reauth && native}
				<Button size="sm" variant="primary" onclick={onreauth}>{m.providers_reauthenticate()}</Button>
			{/if}
			<IconButton icon="settings" label={m.providers_edit()} inline size={14} onclick={onedit} />
			{#if canRemove}
				<IconButton
					icon="x"
					label={m.providers_remove_aria({ provider: providerLabel(p.provider) })}
					inline
					hoverDanger
					size={13}
					onclick={onremove}
				/>
			{/if}
		{/if}
	</div>

	{#if p.needs_reauth}
		<div class="reauth" title={p.last_auth_error ?? undefined}>
			<Text as="span" size="xs">{m.providers_credential_rejected()}</Text>
		</div>
	{/if}

	<UsageBars
		id={p.id}
		provider={p.provider}
		accountId={canManage ? p.account_id : null}
		enabled={usageEnabled}
		softLimits={p.soft_limits}
	/>

	{#if canPin}
		<div class="pin">
			<Text as="span" size="xs" tone="muted" title={m.providers_header_pin_help()}>
				{m.providers_header_pin()}
			</Text>
			<Switch checked={headerPin} label={m.providers_header_pin()} onclick={togglePin} />
		</div>
	{/if}

	<div class="stats">
		<Text as="span" size="xs" tone="faint">{m.providers_requests({ n: compact(p.request_count) })}</Text>
		<Text as="span" size="xs" tone="faint">
			{m.providers_used()} <Timestamp value={p.last_used_at} mode="relative" tone="inherit" />
		</Text>
	</div>
</div>

<style>
	.column {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-3) var(--sp-4);
		border-right: 1px solid var(--border);
		min-width: 0;
	}
	.column:last-child {
		border-right: 0;
	}
	.head {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.summary {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.mark {
		display: inline-flex;
		flex: none;
	}
	.spacer {
		flex: 1;
	}
	.reauth {
		padding: var(--sp-1) var(--sp-2);
		border: 1px solid var(--danger);
		border-radius: var(--r-sm);
		background: color-mix(in srgb, var(--danger) 12%, transparent);
		color: var(--danger);
	}
	.pin {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-3);
	}
	.stats {
		display: flex;
		gap: var(--sp-4);
		margin-top: auto;
		padding-top: var(--sp-1);
	}
</style>
