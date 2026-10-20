<script lang="ts">
	import { renderMarkdown } from '$lib/markdown';
	import type { PermReq } from '$lib/ws.svelte';
	import { Badge, Button, Card, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		req,
		onrespond
	}: {
		req: PermReq;
		onrespond: (rid: string, allow: boolean, optionId?: string) => void;
	} = $props();

	const options = $derived(req.options ?? []);
	const allows = (kind: string) => kind.startsWith('allow');

	// ExitPlanMode fallback: the preview is the tool-input JSON with a `.plan`
	// markdown string. Render it as markdown (mirroring PlanCard) instead of a
	// raw code block; fall back to the raw preview if it doesn't parse.
	const planMarkdown = $derived.by(() => {
		if (req.tool_name !== 'ExitPlanMode' || !req.input_preview) return null;
		try {
			const plan = JSON.parse(req.input_preview)?.plan;
			return typeof plan === 'string' ? plan : null;
		} catch {
			return null;
		}
	});
</script>

<Card tone="attention" padding="sm" gap="var(--sp-2)">
	<div class="row">
		<Badge tone="warn">{m.permission_badge()}</Badge>
		<Text variant="code" weight="semibold" truncate>{req.tool_name}</Text>
	</div>
	{#if req.description}<Text as="p" tone="muted" size="sm">{req.description}</Text>{/if}
	{#if planMarkdown != null}
		<div class="plan-body">{@html renderMarkdown(planMarkdown)}</div>
	{:else if req.input_preview}<pre class="prev mono">{req.input_preview}</pre>{/if}
	<div class="row acts">
		{#if options.length}
			{#each options as o (o.option_id)}
				<Button
					variant={allows(o.kind) ? 'primary' : 'danger'}
					block
					onclick={() => onrespond(req.request_id, allows(o.kind), o.option_id)}>{o.name || o.kind}</Button
				>
			{/each}
		{:else}
			<Button variant="danger" block onclick={() => onrespond(req.request_id, false)}>{m.permission_deny()}</Button>
			<Button variant="primary" block onclick={() => onrespond(req.request_id, true)}>{m.permission_allow()}</Button>
		{/if}
	</div>
</Card>

<style>
	.prev {
		max-height: 8rem;
		overflow-y: auto;
		overflow-x: hidden;
		background: var(--bg);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		padding: var(--sp-2);
		font-size: var(--fs-xs);
		white-space: pre-wrap;
		word-break: break-word;
	}
	.plan-body {
		max-height: 12rem;
		overflow-y: auto;
		overflow-x: hidden;
		background: var(--bg);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		padding: var(--sp-2);
	}
	.acts {
		gap: var(--sp-2);
		flex-wrap: wrap;
	}
</style>
