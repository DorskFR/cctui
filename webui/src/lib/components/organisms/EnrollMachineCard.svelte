<script lang="ts">
	import { apiOrigin } from '$lib/config';
	import { copyText } from '$lib/clipboard';
	import { m } from '$lib/paraglide/messages';
	import { Button, Card, Cluster, Stack, Text } from '@dorsk/tsumikit';

	let { dashed = false }: { dashed?: boolean } = $props();

	const enrollCmd = $derived(
		`cctui-daemon enroll --server-url ${apiOrigin()} --token <user-token> --name "$(hostname)"`
	);

	async function copyEnroll() {
		await copyText(enrollCmd);
	}
</script>

<Card
	padding={dashed ? 'sm' : 'md'}
	style={dashed
		? 'border-style: dashed; border-color: var(--border-strong); background: transparent'
		: undefined}
>
	<Stack>
		<Text weight="bold">{m.home_enroll_title()}</Text>
		<Text as="p" tone="muted" size="sm">
			{m.home_enroll_install_before()} <Text variant="code">cctui-daemon</Text>
			{m.home_enroll_install_after()}
		</Text>
		<Cluster wrap={false} align="center">
			<!-- as="div": truncate needs a block element — text-overflow:ellipsis is
			     ignored on an inline <span>, so the long command would spill. -->
			<div class="cmd"><Text as="div" variant="code" truncate>{enrollCmd}</Text></div>
			<Button onclick={copyEnroll}>{m.common_copy()}</Button>
		</Cluster>
		<Text as="p" tone="muted" size="sm">
			{m.home_enroll_run_as_service()} <Text variant="code">cctui-daemon service install</Text>
		</Text>
	</Stack>
</Card>

<style>
	/* Owns the shrink (min-width:0 + flex:1) that lets the Text inside truncate. */
	.cmd {
		flex: 1;
		min-width: 0;
		padding: var(--sp-2) var(--sp-3);
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		font-size: var(--fs-xs);
	}
</style>
