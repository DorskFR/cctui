<script lang="ts">
	import { Card, Stack } from '@dorsk/tsumikit';
	import AccountsPanel from '$lib/components/organisms/accounts/AccountsPanel.svelte';
	import AccountStats from '$lib/stats/accounts/AccountStats.svelte';
</script>

<Stack gap="var(--sp-5)">
	<AccountsPanel />
	<Card><AccountStats size="full" /></Card>
</Stack>
