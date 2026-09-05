<script lang="ts">
	import type { OAuthAccount } from '$lib/queries';
	import { Fieldset, IconButton, Input, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { ACCOUNT_DRAG_MIME } from './pools.logic';
	import { accountDrag } from './drag.svelte';

	let {
		accounts,
		busy = false,
		oncreate,
		ondiscard
	}: {
		accounts: OAuthAccount[];
		busy?: boolean;
		/** Creates the pool with the typed name and the dropped account, if any. */
		oncreate?: (name: string, accountId: string | null) => void;
		ondiscard?: () => void;
	} = $props();

	let name = $state('');
	const dragged = $derived(accounts.find((a) => a.id === accountDrag.accountId)?.name ?? '');
	const ready = $derived(name.trim().length > 0 && !busy);
</script>

<Fieldset
	tone="strong"
	dashed
	padding="sm"
	droppable
	mime={ACCOUNT_DRAG_MIME}
	accepts={() => ready}
	ondrop={(id) => oncreate?.(name.trim(), id || accountDrag.accountId || null)}
	dropHint={m.pools_drop_hint({ name: dragged })}
	class="new-pool"
>
	{#snippet legend()}
		<Input
			size="sm"
			mono
			width="10rem"
			bind:value={name}
			placeholder={m.pools_name_placeholder()}
			aria-label={m.pools_name()}
			disabled={busy}
			onkeydown={(e: KeyboardEvent) => {
				if (e.key === 'Enter' && ready) oncreate?.(name.trim(), null);
				if (e.key === 'Escape') ondiscard?.();
			}}
		/>
		<IconButton icon="check" label={m.pools_save()} inline size={13} disabled={!ready} onclick={() => oncreate?.(name.trim(), null)} />
		<IconButton icon="x" label={m.pools_discard()} inline hoverDanger size={13} onclick={ondiscard} />
	{/snippet}
	<div class="hint">
		<Text as="p" tone="faint" size="sm">{ready ? m.pools_draft_hint() : m.pools_draft_name_first()}</Text>
	</div>
</Fieldset>

<style>
	.hint {
		display: grid;
		place-items: center;
		min-height: var(--sp-12);
	}
</style>
