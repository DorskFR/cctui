<script lang="ts">
	import { statusBadgeTone } from '$lib/format';
	import { sessionEndTitle } from '$lib/sessionEnd';
	import { Badge, Cluster, Stack, Timestamp } from '@dorsk/tsumikit';
	import Badges from './Badges.svelte';
	import Footer from './Footer.svelte';
	import Lead from './Lead.svelte';
	import { type SessionActions, type SessionView, statusLabel } from './view';

	// Header band (lead group + trailing status · perm · unread · time), a
	// 3-line preview, and the footer pinned to the bottom.
	let { view, actions }: { view: SessionView; actions: SessionActions } = $props();
	const s = $derived(view.s);
</script>

<Stack gap="var(--sp-2)" style="height:100%">
	<Cluster wrap={false} gap="var(--sp-2)" align="flex-start">
		<span class="lead"><Lead {view} {actions} /></span>
		<span class="trail">
			{#if view.showStatusBadge}<Badge tone={statusBadgeTone(s.status)} size="xs">{statusLabel(s.status)}</Badge>{/if}
			{#if view.end}<span class="end-badge" class:end-muted={view.end.muted} title={sessionEndTitle(view.end)}
					><Badge tone={view.end.tone} size="xs">{view.end.badge}</Badge></span
				>{/if}
			<Badges {view} />
			{#if s.last_message_at}<span class="time"
					><Timestamp value={s.last_message_at} mode="relative" tone="faint" size="xs" /></span
				>{/if}
		</span>
	</Cluster>

	{#if s.match_snippet}
		<div class="preview match">🔍 {#if view.snippetHtml}{@html view.snippetHtml}{:else}{s.match_snippet}{/if}</div>
	{:else if view.lastMsg}
		<div class="preview muted">{view.lastMsg}</div>
	{:else}
		<div class="preview"></div>
	{/if}

	<Footer {view} {actions} />
</Stack>

<style>
	.lead {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		column-gap: var(--sp-2);
		row-gap: var(--sp-2);
		flex: 1 1 12rem;
		min-width: 0;
		min-height: 1.75rem;
	}
	/* The trail must never crush the lead: an end reason can be a whole error
	   sentence, and a squeezed lead collapses its badges into circles. */
	.trail {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		flex: 0 1 auto;
		min-width: 0;
		min-height: 1.75rem;
	}
	.time {
		flex: none;
		white-space: nowrap;
	}
	.end-badge {
		display: inline-flex;
		min-width: 0;
		max-width: 22ch;
		overflow: hidden;
	}
	.end-muted {
		opacity: 0.6;
	}
	.preview {
		flex: 1 1 auto;
		min-width: 0;
		min-height: 0;
		overflow: hidden;
		font-size: var(--fs-sm);
		line-height: 1.5;
		white-space: normal;
		display: -webkit-box;
		-webkit-line-clamp: 3;
		line-clamp: 3;
		-webkit-box-orient: vertical;
	}
	.muted {
		color: var(--text-muted);
	}
	.match {
		color: var(--text);
		border-left: 2px solid var(--accent);
		padding-left: var(--sp-2);
	}
</style>
