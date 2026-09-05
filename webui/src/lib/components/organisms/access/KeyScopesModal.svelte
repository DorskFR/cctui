<script lang="ts">
	import { untrack } from 'svelte';
	import { Button, Field, Input, Modal, Switch, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { ALL_SCOPES, type ScopeName } from './access.logic';

	let {
		title,
		label: initialLabel = '',
		scopes: initialScopes = ['read'],
		ceiling,
		help = m.access_key_scopes_help(),
		withLabel = false,
		saveLabel = m.common_save(),
		onsave,
		onclose
	}: {
		title: string;
		label?: string | null;
		scopes?: readonly string[];
		/** Scopes the owner may delegate; anything outside stays disabled. */
		ceiling: ReadonlySet<string>;
		help?: string;
		withLabel?: boolean;
		saveLabel?: string;
		onsave: (label: string | null, scopes: string[]) => void;
		onclose: () => void;
	} = $props();

	let label = $state(untrack(() => initialLabel) ?? '');
	let picked = $state(new Set<ScopeName>(untrack(() => initialScopes) as ScopeName[]));

	function toggle(s: ScopeName) {
		const next = new Set(picked);
		if (next.has(s)) next.delete(s);
		else next.add(s);
		picked = next;
	}
	function save() {
		onsave(label.trim() || null, [...picked]);
		onclose();
	}
</script>

<Modal {title} size="sm" {onclose}>
	{#snippet body()}
		{#if withLabel}
			<Field label={m.users_field_label_optional()}>
				<Input mono placeholder={m.users_label_optional_placeholder()} bind:value={label} />
			</Field>
		{/if}
		<Text as="p" size="sm" tone="faint">{help}</Text>
		<div class="scopes">
			{#each ALL_SCOPES as s (s)}
				<Switch
					checked={picked.has(s)}
					label={s}
					disabled={!ceiling.has(s)}
					title={ceiling.has(s) ? m.users_scope_grant({ scope: s }) : m.users_scope_not_in_ceiling()}
					onclick={() => toggle(s)}
				/>
			{/each}
		</div>
	{/snippet}
	{#snippet footer()}
		<Button onclick={onclose}>{m.common_cancel()}</Button>
		<Button variant="primary" disabled={picked.size === 0} onclick={save}>{saveLabel}</Button>
	{/snippet}
</Modal>

<style>
	.scopes {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-3);
		margin-top: var(--sp-2);
	}
</style>
