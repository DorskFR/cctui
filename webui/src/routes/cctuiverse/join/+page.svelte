<script lang="ts">
	import { Callout, CopyButton, Input, Stack, Text } from '@dorsk/tsumikit';
	import PageHead from '$lib/components/molecules/PageHead.svelte';
	import { findInvite } from '$lib/cctuiverse';
	import { m } from '$lib/paraglide/messages';

	const invite = findInvite(window.location.href);
</script>

<PageHead title={m.cctuiverse_join_page_title()} />

<Stack gap="var(--sp-3)">
	<Text tone="muted">{m.cctuiverse_join_page_intro()}</Text>
	{#if invite}
		<div class="url">
			<Input value={invite} readonly mono grow aria-label={m.cctuiverse_invite_url()} />
			<CopyButton text={invite} label={m.common_copy()} />
		</div>
	{:else}
		<Callout tone="warn">{m.cctuiverse_join_page_incomplete()}</Callout>
	{/if}
	<Text size="sm">{m.cctuiverse_join_page_how()}</Text>
	<Callout tone="info">{m.cctuiverse_join_page_never()}</Callout>
</Stack>

<style>
	.url {
		display: flex;
		gap: var(--sp-1);
		align-items: center;
	}
</style>
