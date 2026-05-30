<script lang="ts">
	import type { PermReq } from '$lib/ws.svelte';

	let {
		req,
		onrespond
	}: { req: PermReq; onrespond: (rid: string, allow: boolean) => void } = $props();
</script>

<div class="perm">
	<div class="row">
		<span class="badge badge-warn">permission</span>
		<span class="tool mono truncate">{req.tool_name}</span>
	</div>
	{#if req.description}<p class="desc muted">{req.description}</p>{/if}
	{#if req.input_preview}<pre class="prev mono">{req.input_preview}</pre>{/if}
	<div class="row acts">
		<button class="btn btn-danger btn-block" onclick={() => onrespond(req.request_id, false)}>
			Deny
		</button>
		<button class="btn btn-primary btn-block" onclick={() => onrespond(req.request_id, true)}>
			Allow
		</button>
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
	.tool {
		font-weight: var(--fw-semibold);
	}
	.desc {
		font-size: var(--fs-sm);
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
