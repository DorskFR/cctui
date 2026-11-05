<script lang="ts">
	import { renderMarkdown } from '$lib/markdown';
	import type { PermReq } from '$lib/ws.svelte';
	import { Button, Card, Text } from '@dorsk/tsumikit';
	import { permissionArgs } from './PermissionCard.logic';
	import { m } from '$lib/paraglide/messages';

	let {
		req,
		onrespond
	}: {
		req: PermReq;
		onrespond: (rid: string, allow: boolean, optionId?: string) => void;
	} = $props();

	const allows = (kind: string) => kind.startsWith('allow');

	// Safe choices first, the one-shot allow last as the only primary.
	const options = $derived.by(() => {
		const all = req.options ?? [];
		const primary =
			all.find((o) => o.kind === 'allow_once') ?? all.find((o) => allows(o.kind)) ?? null;
		const rejects = all.filter((o) => !allows(o.kind));
		const others = all.filter((o) => allows(o.kind) && o !== primary);
		return [
			...rejects.map((o) => ({ o, variant: 'ghost' as const })),
			...others.map((o) => ({ o, variant: 'default' as const })),
			...(primary ? [{ o: primary, variant: 'primary' as const }] : [])
		];
	});

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

	const args = $derived(planMarkdown === null ? permissionArgs(req.input_preview) : null);
	const description = $derived(
		req.description && req.description.trim() !== req.tool_name ? req.description : null
	);
</script>

<Card tone="attention" padding="sm" gap="var(--sp-2)">
	<div class="row head">
		<Text variant="eyebrow" tone="muted">{m.permission_badge()}</Text>
		<Text mono weight="semibold" truncate>{req.tool_name}</Text>
	</div>
	{#if description}<Text as="p" tone="muted" size="sm">{description}</Text>{/if}
	{#if planMarkdown != null}
		<div class="plan-body">{@html renderMarkdown(planMarkdown)}</div>
	{:else if args}
		{#if args.description}<Text as="p" size="sm">{args.description}</Text>{/if}
		{#if args.command}<pre class="prev mono">{args.command}</pre>{/if}
		{#if args.fields.length}
			<dl class="fields">
				{#each args.fields as [k, v] (k)}
					<dt>{k}</dt>
					<dd class="mono">{v}</dd>
				{/each}
			</dl>
		{/if}
	{:else if req.input_preview}<pre class="prev mono">{req.input_preview}</pre>{/if}
	<div class="row acts">
		{#if options.length}
			{#each options as { o, variant } (o.option_id)}
				<Button {variant} onclick={() => onrespond(req.request_id, allows(o.kind), o.option_id)}
					>{o.name || o.kind}</Button
				>
			{/each}
		{:else}
			<Button variant="ghost" onclick={() => onrespond(req.request_id, false)}>{m.permission_deny()}</Button>
			<Button variant="primary" onclick={() => onrespond(req.request_id, true)}>{m.permission_allow()}</Button>
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
	.head {
		gap: var(--sp-2);
		align-items: baseline;
	}
	.fields {
		display: grid;
		grid-template-columns: auto 1fr;
		gap: var(--sp-1) var(--sp-3);
		margin: 0;
		font-size: var(--fs-xs);
	}
	.fields dt {
		color: var(--text-muted);
	}
	.fields dd {
		margin: 0;
		white-space: pre-wrap;
		word-break: break-word;
		max-height: 8rem;
		overflow-y: auto;
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
		justify-content: flex-end;
	}
</style>
