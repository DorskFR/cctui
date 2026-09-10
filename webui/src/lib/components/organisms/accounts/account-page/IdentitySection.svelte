<script lang="ts">
	import { untrack } from 'svelte';
	import { errMessage } from '$lib/api';
	import { useAccountActions, type OAuthAccount, type UpdateAccount } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import AccountAvatar from '$lib/components/molecules/AccountAvatar.svelte';
	import EmojiPicker from '$lib/components/molecules/EmojiPicker.svelte';
	import { isValidAccountEmoji } from '$lib/components/molecules/avatar';
	import { Button, Card, Field, Input, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let { account, owner = null }: { account: OAuthAccount; owner?: string | null } = $props();

	const actions = useAccountActions();
	// Seeded once: a refetch must not overwrite what the operator is typing.
	let name = $state(untrack(() => account.name));
	let emoji = $state(untrack(() => account.emoji ?? ''));
	let saving = $state(false);

	const emojiOk = $derived(isValidAccountEmoji(emoji));
	const dirty = $derived(name !== account.name || emoji !== (account.emoji ?? ''));

	async function save() {
		if (!name.trim()) {
			toasts.error(m.accounts_err_name_required());
			return;
		}
		if (!emojiOk) {
			toasts.error(m.account_emoji_invalid());
			return;
		}
		saving = true;
		try {
			const body: UpdateAccount = { name: name.trim(), emoji: emoji.trim() };
			await actions.update(account.id, body);
			toasts.ok(m.accounts_account_updated());
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			saving = false;
		}
	}
</script>

<Card title={m.account_section_identity()} subtitle={m.account_section_identity_help()}>
	{#snippet actions()}
		<Button size="sm" variant="primary" disabled={!dirty || saving} onclick={save}>
			{m.common_save()}
		</Button>
	{/snippet}

	<div class="body">
		<Field label={m.accounts_field_name()}>
			<Input bind:value={name} placeholder={m.accounts_field_name_placeholder()} />
		</Field>

		<Field label={m.account_emoji_label()}>
			<div class="emoji">
				<AccountAvatar {emoji} {name} id={account.id} size={24} />
				<EmojiPicker value={emoji} onselect={(v) => (emoji = v)} />
				<Input
					bind:value={emoji}
					placeholder={m.account_emoji_placeholder()}
					maxlength={16}
					aria-label={m.account_emoji_label()}
					style="max-width: 8rem"
				/>
				<Button control onclick={() => (emoji = '')} disabled={!emoji}>
					{m.account_emoji_clear()}
				</Button>
			</div>
			{#if !emojiOk}
				<Text tone="danger" size="xs">{m.account_emoji_invalid()}</Text>
			{:else}
				<Text tone="faint" size="xs">{m.account_emoji_hint()}</Text>
			{/if}
		</Field>

		{#if owner}
			<Field label={m.accounts_field_owner()}>
				<Text as="div" size="sm">{owner}</Text>
			</Field>
		{/if}
	</div>
</Card>

<style>
	.body {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.emoji {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		flex-wrap: wrap;
	}
</style>
