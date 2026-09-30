<script lang="ts">
	import { Button, Modal, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import type { LimitResetEntry } from '$lib/queries';
	import {
		resetExpiry,
		resetReason,
		resetRestores,
		resetTitle
	} from '$lib/components/molecules/limit-reset';

	let {
		entries = [],
		claiming = null,
		onclaim
	}: {
		entries?: LimitResetEntry[];
		/** The entry id a claim is in flight for. */
		claiming?: string | null;
		onclaim: (entry: LimitResetEntry) => void;
	} = $props();

	let confirming = $state<LimitResetEntry | null>(null);

	/** A program with no grant to spend and no expiry to name: a greyed row, not
	 *  a dead button. */
	const isEmptyProgram = (e: LimitResetEntry) => !e.usable && !e.expires_at;

	function confirm() {
		const entry = confirming;
		confirming = null;
		if (entry) onclaim(entry);
	}
</script>

<div class="page">
	<Text as="p" tone="muted" size="sm">{m.limit_resets_intro()}</Text>

	{#if entries.length === 0}
		<Text as="p" tone="faint" size="sm">{m.limit_resets_empty()}</Text>
	{/if}

	{#each entries as entry (entry.id)}
		{@const restores = resetRestores(entry)}
		{@const expiry = resetExpiry(entry)}
		{@const reason = resetReason(entry)}
		<div class="row" class:spent={isEmptyProgram(entry)}>
			<div class="what">
				<Text as="div" size="sm" weight="medium">{resetTitle(entry)}</Text>
				<div class="meta">
					{#if isEmptyProgram(entry)}
						<Text as="span" size="xs" tone="faint">{m.limit_resets_none()}</Text>
					{:else}
						{#if restores}
							<Text as="span" size="xs" tone="muted">{m.sessions_limit_reset_clears({ windows: restores })}</Text>
						{/if}
						{#if expiry}
							<Text as="span" size="xs" tone="muted">{expiry}</Text>
						{/if}
						{#if entry.resets_left !== null && entry.resets_left > 1}
							<Text as="span" size="xs" tone="muted">{m.limit_resets_claims_left({ n: entry.resets_left })}</Text>
						{/if}
					{/if}
				</div>
			</div>
			{#if !isEmptyProgram(entry)}
				<Button
					size="sm"
					tone="warn"
					disabled={!entry.usable || claiming !== null}
					loading={claiming === entry.id}
					title={reason}
					onclick={() => (confirming = entry)}
				>
					{m.limit_resets_claim()}
				</Button>
			{/if}
		</div>
		{#if reason && !isEmptyProgram(entry)}
			<Text as="p" size="xs" tone="faint">{reason}</Text>
		{/if}
	{/each}
</div>

{#if confirming}
	{@const entry = confirming}
	<Modal
		title={m.sessions_limit_reset_confirm_title()}
		tone="warn"
		size="sm"
		onclose={() => (confirming = null)}
	>
		{#snippet body()}
			<Text>{m.sessions_limit_reset_confirm_body({ title: resetTitle(entry) })}</Text>
			{#if resetRestores(entry)}
				<Text tone="muted">{m.sessions_limit_reset_clears({ windows: resetRestores(entry) })}</Text>
			{/if}
		{/snippet}
		{#snippet footer()}
			<Button variant="ghost" onclick={() => (confirming = null)}>{m.sessions_limit_reset_cancel()}</Button>
			<Button tone="warn" onclick={confirm}>{m.sessions_limit_reset_confirm()}</Button>
		{/snippet}
	</Modal>
{/if}

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.row {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		padding-bottom: var(--sp-3);
		border-bottom: 1px solid var(--border);
	}
	.row.spent {
		opacity: 0.6;
	}
	.what {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		min-width: 0;
		flex: 1;
	}
	.meta {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-1) var(--sp-3);
	}
</style>
