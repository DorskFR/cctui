<script lang="ts">
	import { getContext } from 'svelte';
	import { Button, Divider, Text } from '@dorsk/tsumikit';
	import type { HostContext, PaneProps } from '../../../../plugin-sdk/types';

	let { session, composer, params, onclose }: PaneProps = $props();
	const host = getContext<HostContext | undefined>('cctui:host');
	let count = $state(0);
</script>

<section class="demo" data-journey="demo-pane" data-word={params.word ?? ''} data-host-origin={host?.origin ?? ''}>
	<Text weight="semibold">Demo pane</Text>
	<Text size="sm" data-journey="demo-session">{session.id}</Text>
	<Divider spacing="12px" />
	<Button data-journey="demo-count" onclick={() => (count += 1)}>count {count}</Button>
	<Button variant="ghost" data-journey="demo-insert" onclick={() => composer.insertText(`[demo] ${params.word}`)}>insert</Button>
	<Button variant="ghost" data-journey="demo-close" onclick={onclose}>close</Button>
</section>

<style>
	.demo {
		display: flex;
		flex-direction: column;
		gap: 8px;
		padding: 12px;
	}
</style>
