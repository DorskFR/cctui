<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { relativeTime, uptime, statusBadgeClass } from '$lib/format';
	import MachineBadge from './MachineBadge.svelte';
	import TokenUsage from './TokenUsage.svelte';
	import AdapterIcon from './AdapterIcon.svelte';

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
	// makes every child read the same ("cctui"). Give nameless subagents the
	// short id (the adjacent "subagent" badge already labels the kind), so
	// siblings are distinguishable without a redundant "subagent ·" prefix.
	const title = $derived(s.name || (child ? s.id.slice(0, 6) : dirName || s.id));
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
	onclick={() => onopen(s)}
>
	<div class="row top">
		{#if child}<span class="sub" title="subagent">↳</span>{/if}
		<span class="dot {livenessClass}"></span>
		<span class="title truncate">{title}</span>
		{#if child}<span class="badge badge-info tag">subagent</span>{/if}
		{#if needsInput}<span class="hand" title="needs input">✋</span>{/if}
		{#if pendingCount > 0}<span class="badge badge-warn">{pendingCount} perm</span>{/if}
		{#if dense && s.last_message_text}
			<span class="preview muted">{s.last_message_text}</span>
		{:else}
			<div class="spacer"></div>
		{/if}
		{#if dense}
			<span class="badge {statusBadgeClass(s.status)} tag">{s.status}</span>
			{#if s.last_message_at}<span class="faint sm">{relativeTime(s.last_message_at)}</span>{/if}
		{/if}
		<AdapterIcon adapter={s.adapter_id} size={16} />
	</div>

	{#if !dense}
		<div class="row meta row-wrap">
			<MachineBadge name={s.machine_name} id={s.machine_id} />
			<span class="badge {statusBadgeClass(s.status)}">{s.status}</span>
			{#if s.model}<span class="muted sm">{s.model}{s.effort ? ` · ${s.effort}` : ''}</span>{/if}
			<span class="muted sm">up {uptime(Number(s.uptime_secs))}</span>
		</div>

		{#if s.last_message_text}
			<div class="last truncate muted">{s.last_message_text}</div>
		{/if}

		<div class="row foot">
			<TokenUsage usage={u} />
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
	.meta {
		gap: var(--sp-2);
	}
	.sm {
		font-size: var(--fs-xs);
	}
	.last {
		font-size: var(--fs-sm);
	}
	/* Dense-mode last-message preview: fills the space between the title and the
	   status badge, single line, ellipsis — never wraps or grows the row. */
	.preview {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: var(--fs-xs);
	}
</style>
