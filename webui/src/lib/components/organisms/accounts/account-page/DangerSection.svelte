<script lang="ts">
	import type { OAuthAccount } from '$lib/queries';
	import { Button, Card, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		account,
		disabled = false,
		ondelete
	}: { account: OAuthAccount; disabled?: boolean; ondelete?: () => void } = $props();
</script>

<Card title={m.account_section_danger()} tone="danger">
	<div class="row">
		<Text as="p" tone="muted" size="sm" measure="60ch">
			{m.account_delete_help({ name: account.name })}
		</Text>
		<span class="spacer"></span>
		<Button size="sm" variant="danger" {disabled} onclick={() => ondelete?.()}>
			{m.accounts_delete_account()}
		</Button>
	</div>
</Card>

<style>
	.row {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: var(--sp-3);
	}
	.spacer {
		flex: 1;
	}
</style>
