<script lang="ts">
	// Trailing rows under a bubble: reply duration + per-reply token breakdown
	// (no Σ; that's the conversation-wide aggregate), stop hook, file edits.
	import TokenUsage from '$lib/components/molecules/TokenUsage.svelte';
	import { Text } from '@dorsk/tsumikit';
	import type { Line } from './types';

	let { ln }: { ln: Line } = $props();

	function durationLabel(ms: number | undefined): string {
		if (!ms || ms < 1000) return '';
		const secs = Math.round(ms / 1000);
		if (secs < 60) return `${secs}s`;
		const mins = Math.floor(secs / 60);
		return `${mins}m ${secs % 60}s`;
	}
</script>

{#if (ln.durationMs || ln.usage) && (ln.role === 'assistant' || ln.role === 'result')}
	{@const dur = durationLabel(ln.durationMs)}
	<div class="line-foot row">
		{#if dur}<Text tone="faint" size="xs">⏱ {dur}</Text>{/if}
		{#if ln.usage}<TokenUsage usage={ln.usage} showSum={false} />{/if}
	</div>
{/if}
{#if ln.stopHook}
	<div class="line-foot row"><Text tone="faint" size="xs">⏹ {ln.stopHook}</Text></div>
{/if}
{#if ln.fileHistory?.length}
	<div class="line-foot row">
		<Text tone="faint" size="xs" title={ln.fileHistory.join('\n')}
			>✎ {ln.fileHistory.length}</Text
		>
	</div>
{/if}

<style>
	/* Layout only; typography (faint xs) is the Text atom's. */
	.line-foot {
		align-self: flex-end;
		padding-inline: var(--sp-1);
	}
</style>
