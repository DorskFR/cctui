<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { compact, relativeTime, uptime, hashHue, statusBadgeClass } from '$lib/format';
	import BrandLogo from './BrandLogo.svelte';

	let {
		session,
		child = false,
		compact: dense = false,
		pendingCount = 0,
		onopen
	}: {
		session: SessionListItem;
		child?: boolean;
		compact?: boolean;
		pendingCount?: number;
		onopen: (s: SessionListItem) => void;
	} = $props();

	const s = $derived(session);
	const dirName = $derived(s.working_dir.split('/').filter(Boolean).pop() || '');
	// Subagents inherit the parent's working dir, so the dir-basename fallback
	// makes every child read the same ("cctui"). Give nameless subagents a
	// clearer label so siblings are distinguishable.
	const title = $derived(
		s.name || (child ? `subagent · ${s.id.slice(0, 6)}` : dirName || s.id)
	);
	const machineLabel = $derived(s.machine_name || s.machine_id.slice(0, 8));
	const isCodex = $derived((s.adapter_id ?? 'claude-code').toString().startsWith('codex'));
	const needsInput = $derived(s.attention === 'needs_input' && s.status !== 'archived');
	const livenessClass = $derived(
		s.liveness === 'active' ? 'dot-active' : s.liveness === 'stale' ? 'dot-stale' : 'dot-dead'
	);
	const u = $derived(s.token_usage);
</script>

<button
	class="card card-tap sc stack"
	class:child
	class:dense
	class:attn={needsInput}
	style={`--mh:${hashHue(machineLabel)}`}
	onclick={() => onopen(s)}
>
	<div class="row top">
		{#if child}<span class="sub" title="subagent">↳</span>{/if}
		<span class="dot {livenessClass}"></span>
		<span class="title truncate">{title}</span>
		{#if child}<span class="badge badge-info tag">subagent</span>{/if}
		{#if needsInput}<span class="hand" title="needs input">✋</span>{/if}
		{#if pendingCount > 0}<span class="badge badge-warn">{pendingCount} perm</span>{/if}
		<div class="spacer"></div>
		{#if dense}
			<span class="badge {statusBadgeClass(s.status)} tag">{s.status}</span>
			{#if s.last_message_at}<span class="faint sm">{relativeTime(s.last_message_at)}</span>{/if}
		{/if}
		<span class="adapter" class:codex={isCodex} title={String(s.adapter_id ?? 'claude-code')}>
			<BrandLogo adapter={s.adapter_id} size={16} />
		</span>
	</div>

	{#if !dense}
		<div class="row meta row-wrap">
			<span class="badge mach">{machineLabel}</span>
			<span class="badge {statusBadgeClass(s.status)}">{s.status}</span>
			{#if s.model}<span class="muted sm">{s.model}{s.effort ? ` · ${s.effort}` : ''}</span>{/if}
			<span class="muted sm">up {uptime(Number(s.uptime_secs))}</span>
		</div>

		{#if s.last_message_text}
			<div class="last truncate muted">{s.last_message_text}</div>
		{/if}

		<div class="row foot">
			<span class="tokens mono faint">
				↑{compact(Number(u.tokens_in))} ↓{compact(Number(u.tokens_out))}
				{#if Number(u.cache_read_tokens) > 0}⚡{compact(Number(u.cache_read_tokens))}{/if}
			</span>
			<div class="spacer"></div>
			{#if s.last_message_at}<span class="faint sm">{relativeTime(s.last_message_at)}</span>{/if}
		</div>
	{/if}
</button>

<style>
	.sc {
		gap: var(--sp-2);
		text-align: left;
		width: 100%;
	}
	.sc.child {
		/* width:auto so the indent margin doesn't push the card past the right
		   edge (the base .sc is width:100%). */
		width: auto;
		margin-left: var(--sp-4);
		border-left: 2px solid var(--border-strong);
		/* slightly recessed/faded vs a top-level card — theme-token based */
		background: color-mix(in srgb, var(--bg) 55%, var(--bg-elevated));
		color: var(--text-muted);
	}
	.sc.dense {
		padding: var(--sp-2) var(--sp-3);
		gap: 0;
	}
	/* Compact mode is a flat list — no indent for subagents. */
	.sc.dense.child {
		margin-left: 0;
		border-left: none;
	}
	.tag {
		font-size: var(--fs-xs);
		padding: 0.05rem var(--sp-2);
	}
	.sc.attn {
		background: var(--attention-bg);
		border-left: 3px solid var(--attention-bar);
	}
	.top {
		gap: var(--sp-2);
	}
	.sub {
		color: var(--text-faint);
	}
	.title {
		font-weight: var(--fw-semibold);
		font-size: var(--fs-md);
	}
	.hand {
		font-size: var(--fs-sm);
	}
	.adapter {
		display: inline-flex;
		align-items: center;
		/* Anthropic = warm/orange, Codex = teal-blue, matching brand hues. */
		color: var(--c-amber);
	}
	.adapter.codex {
		color: var(--c-blue);
	}
	.meta {
		gap: var(--sp-2);
	}
	.sm {
		font-size: var(--fs-xs);
	}
	.mach {
		background: hsl(var(--mh) 45% 22%);
		color: hsl(var(--mh) 70% 80%);
		border-color: hsl(var(--mh) 45% 35%);
	}
	.last {
		font-size: var(--fs-sm);
	}
	.foot .tokens {
		font-size: var(--fs-xs);
	}
</style>
