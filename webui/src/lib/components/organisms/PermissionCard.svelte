<script lang="ts">
	import type { PermReq } from '$lib/ws.svelte';
	import Badge from '$lib/components/atoms/Badge.svelte';
	import Button from '$lib/components/atoms/Button.svelte';
	import Text from '$lib/components/atoms/Text.svelte';

	let {
		req,
		onrespond
	}: { req: PermReq; onrespond: (rid: string, allow: boolean) => void } = $props();
</script>

<div class="perm">
	<div class="row">
		<Badge tone="warn">permission</Badge>
		<Text variant="code" weight="semibold" truncate>{req.tool_name}</Text>
	</div>
	{#if req.description}<Text as="p" tone="muted" size="sm">{req.description}</Text>{/if}
	{#if req.input_preview}<pre class="prev mono">{req.input_preview}</pre>{/if}
	<div class="row acts">
		<Button variant="danger" block onclick={() => onrespond(req.request_id, false)}>Deny</Button>
		<Button variant="primary" block onclick={() => onrespond(req.request_id, true)}>Allow</Button>
	</div>
</div>

<style>
	.perm {
		background: var(--attention-bg);
		border: 1px solid var(--attention-bar);
		border-radius: var(--r-md);
		padding: var(--sp-3);
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.prev {
		max-height: 8rem;
		overflow: auto;
		background: var(--bg);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		padding: var(--sp-2);
		font-size: var(--fs-xs);
		white-space: pre-wrap;
		word-break: break-word;
	}
	.acts {
		gap: var(--sp-2);
	}
</style>
