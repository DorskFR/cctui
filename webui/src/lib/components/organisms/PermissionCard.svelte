<script lang="ts">
	import { renderMarkdown } from '$lib/markdown';
	import type { PermReq } from '$lib/ws.svelte';
	import { Badge, Button, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		req,
		onrespond
	}: { req: PermReq; onrespond: (rid: string, allow: boolean) => void } = $props();

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

<div class="perm">
	<div class="row">
		<Badge tone="warn">{m.permission_badge()}</Badge>
		<Text variant="code" weight="semibold" truncate>{req.tool_name}</Text>
	</div>
	{#if req.description}<Text as="p" tone="muted" size="sm">{req.description}</Text>{/if}
	{#if planMarkdown != null}
		<div class="plan-body">{@html renderMarkdown(planMarkdown)}</div>
	{:else if req.input_preview}<pre class="prev mono">{req.input_preview}</pre>{/if}
	<div class="row acts">
		<Button variant="danger" block onclick={() => onrespond(req.request_id, false)}>{m.permission_deny()}</Button>
		<Button variant="primary" block onclick={() => onrespond(req.request_id, true)}>{m.permission_allow()}</Button>
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
	.plan-body {
		max-height: 12rem;
		overflow: auto;
		background: var(--bg);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		padding: var(--sp-2);
	}
	.acts {
		gap: var(--sp-2);
	}
</style>
