<script lang="ts">
	import { apiOrigin } from '$lib/config';
	import { copyText } from '$lib/clipboard';
	import { m } from '$lib/paraglide/messages';
	import InlineCode, { SLOT } from '$lib/components/atoms/InlineCode.svelte';
	import { Button, Card, Stack, Text } from '@dorsk/tsumikit';

	const enrollCmd = $derived(
		`cctui-daemon enroll --server-url ${apiOrigin()} --token <user-token> --name "$(hostname)"`
	);

	async function copyEnroll() {
		await copyText(enrollCmd);
	}
</script>

<Card padding="md">
	<Stack data-journey="enroll">
		<Text weight="bold">{m.home_enroll_title()}</Text>
		<Text as="p" tone="muted" size="sm">
			<InlineCode text={m.home_enroll_install({ daemon: SLOT })} code="cctui-daemon" />
		</Text>
		<div class="cmd">
			<pre data-journey="enroll-cmd"><code>{enrollCmd}</code></pre>
			<Button size="sm" data-journey="enroll-copy" onclick={copyEnroll}>{m.common_copy()}</Button>
		</div>
		<Text as="p" tone="muted" size="sm">
			{m.home_enroll_run_as_service()} <Text variant="code">cctui-daemon service install</Text>
		</Text>
	</Stack>
</Card>

<style>
	.cmd {
		display: flex;
		align-items: flex-start;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		background: var(--bg);
		border-radius: var(--r-md);
	}
	pre {
		flex: 1;
		min-width: 0;
		margin: 0;
		font-family: var(--font-mono);
		font-size: var(--fs-xs);
		line-height: var(--lh-snug, 1.4);
		white-space: pre-wrap;
		overflow-wrap: anywhere;
	}
</style>
