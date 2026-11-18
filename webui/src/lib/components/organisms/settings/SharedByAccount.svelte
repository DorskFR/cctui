<script lang="ts">
	import { Text } from '@dorsk/tsumikit';
	import type { OAuthAccount } from '@bindings/OAuthAccount';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import { useAccountShares } from '#lib/queries/index.ts';
	import { m } from '#lib/paraglide/messages.js';

	let { account }: { account: OAuthAccount } = $props();

	const shares = useAccountShares(() => account.id);
	const names = $derived((shares.data ?? []).map((s) => s.user_name || s.user_id));
</script>

{#if names.length > 0}
	<SettingRow label={account.emoji ? `${account.emoji} ${account.name}` : account.name} selfLabelled journey="shared-by-me" journeyKey={account.id}>
		<Text size="sm" tone="faint">{m.settings_sharing_with({ names: names.join(', ') })}</Text>
	</SettingRow>
{/if}
