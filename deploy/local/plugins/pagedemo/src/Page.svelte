<script lang="ts">
	import { getContext } from 'svelte';
	import { Button, Divider, Text } from '@dorsk/tsumikit';
	import type { HostContext, PageProps } from '../../../../../webui/plugin-sdk/types';

	let { basePath, path, navigate }: PageProps = $props();
	const host = getContext<HostContext | undefined>('cctui:host');

	const ITEMS = ['alpha', 'beta', 'gamma'];
	const detail = $derived(path.startsWith('/item/') ? path.slice('/item/'.length) : null);
</script>

<section class="page" data-journey="pagedemo" data-base={basePath} data-path={path} data-host-origin={host?.origin ?? ''}>
	<Text weight="semibold">Page demo</Text>
	<Text size="sm" tone="faint" data-journey="pagedemo-path">{path}</Text>
	<Divider spacing="12px" />
	{#if detail}
		<Text data-journey="pagedemo-detail">{detail}</Text>
		<Button variant="ghost" data-journey="pagedemo-back" onclick={() => navigate('/')}>back to the list</Button>
	{:else}
		<ul data-journey="pagedemo-list">
			{#each ITEMS as item (item)}
				<li>
					<Button variant="ghost" data-journey="pagedemo-item" data-journey-key={item} onclick={() => navigate(`/item/${item}`)}>
						{item}
					</Button>
				</li>
			{/each}
		</ul>
	{/if}
</section>

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: 8px;
		padding: 16px;
	}
	ul {
		display: flex;
		flex-direction: column;
		gap: 4px;
		margin: 0;
		padding: 0;
		list-style: none;
	}
</style>
