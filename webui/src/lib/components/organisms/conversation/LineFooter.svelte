<script lang="ts">
	// Trailing rows under a bubble: reply duration + per-reply token breakdown
	// (no total; that's the conversation-wide aggregate), stop hook, file edits.
	import StatText from '$lib/components/atoms/StatText.svelte';
	import { tokenUsageLayout } from '$lib/components/molecules/TokenUsage.logic';
	import { Text } from '@dorsk/tsumikit';
	import type { Line } from './types';
	import { m } from '$lib/paraglide/messages';

	let { ln }: { ln: Line } = $props();

	function durationLabel(ms: number | undefined): string {
		if (!ms || ms < 1000) return '';
		const secs = Math.round(ms / 1000);
		if (secs < 60) return `${secs}s`;
		const mins = Math.floor(secs / 60);
		return `${mins}m ${secs % 60}s`;
	}

	const layout = $derived(ln.usage ? tokenUsageLayout(ln.usage, { showSum: false }) : null);
</script>

{#if (ln.durationMs || ln.usage) && (ln.role === 'assistant' || ln.role === 'result')}
	{@const dur = durationLabel(ln.durationMs)}
	<div class="line-foot row">
		{#if dur}<Text tone="faint" size="xs" numeric>{dur}</Text>{/if}
		{#if ln.usage && layout}
			<StatText value={Number(ln.usage.tokens_in)} kind="input" />
			<StatText value={Number(ln.usage.tokens_out)} kind="output" />
			{#if layout.showCache}<StatText value={layout.cacheTotal} kind="cache" />{/if}
			{#if layout.showCost}<StatText value={layout.cost} kind="cost" />{/if}
		{/if}
	</div>
{/if}
{#if ln.stopHook}
	<div class="line-foot row"><Text tone="faint" size="xs">{m.tx_stop_hook({ hook: ln.stopHook })}</Text></div>
{/if}
{#if ln.fileHistory?.length}
	<div class="line-foot row" title={ln.fileHistory.join('\n')}>
		<StatText value={ln.fileHistory.length} kind="files" />
	</div>
{/if}

<style>
	.line-foot {
		align-self: flex-end;
		padding-inline: var(--sp-1);
		gap: var(--sp-2);
		font-size: var(--fs-xs);
		color: var(--text-faint);
	}
</style>
