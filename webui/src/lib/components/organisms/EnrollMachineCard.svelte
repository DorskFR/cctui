<script lang="ts">
	import { apiOrigin } from '#lib/config.ts';
	import { copyText } from '#lib/clipboard.ts';
	import { m } from '#lib/paraglide/messages.js';
	import InlineCode, { SLOT } from '#lib/components/atoms/InlineCode.svelte';
	import { Button, Card, Stack, Text } from '@dorsk/tsumikit';
	import { hasFeature } from '#lib/features.svelte.ts';

	const enrollCmd = $derived(
		`cctui-daemon enroll --server-url ${apiOrigin()} --token <user-token> --name "$(hostname)"`
	);

	async function copyEnroll() {
		await copyText(enrollCmd);
	}
</script>

{#if hasFeature('machine_spawn')}
	<Card padding="md">
		<Stack data-journey="enroll">
			<Text weight="bold">{m.home_enroll_title()}</Text>
			<Text as="p" tone="muted" size="sm">
				<InlineCode text={m.home_enroll_install({ daemon: SLOT })} code="cctui-daemon" />
			</Text>
			<div class="cmd" data-journey="enroll-cmd">
				<div class="cmd-text"><Text as="div" variant="code">{enrollCmd}</Text></div>
				<Button size="sm" data-journey="enroll-copy" onclick={copyEnroll}>{m.common_copy()}</Button>
			</div>
			<Text as="p" tone="muted" size="sm">
				{m.home_enroll_run_as_service()} <Text variant="code">cctui-daemon service install</Text>
			</Text>
		</Stack>
	</Card>
{/if}

<style>
	.cmd {
		display: flex;
		align-items: flex-start;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		background: var(--bg);
		border-radius: var(--r-md);
		font-size: var(--fs-xs);
		white-space: pre-wrap;
		overflow-wrap: anywhere;
	}
	.cmd-text {
		flex: 1;
		min-width: 0;
	}
</style>
