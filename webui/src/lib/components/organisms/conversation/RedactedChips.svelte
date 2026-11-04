<script lang="ts">
	import { Button, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { hasUnclosedTag, redactedSpans } from '$lib/redactTag';

	let { text }: { text: string } = $props();

	const spans = $derived(redactedSpans(text));
	const unclosed = $derived(hasUnclosedTag(text));
	let revealed = $state<Set<string>>(new Set());

	function toggle(value: string) {
		const next = new Set(revealed);
		if (next.has(value)) next.delete(value);
		else next.add(value);
		revealed = next;
	}
</script>

{#if spans.length || unclosed}
	<div class="redacted" data-testid="redacted-chips">
		{#if spans.length}
			<Text size="xs" tone="faint">{m.redacted_tag_chips_label()}</Text>
			{#each spans as span (span.start)}
				{@const open = revealed.has(span.value)}
				<Button
					variant="ghost"
					size="sm"
					chip
					pressed={open}
					data-testid="redacted-chip"
					title={open ? m.redacted_tag_chip_hide() : m.redacted_tag_chip_reveal()}
					aria-label={open ? m.redacted_tag_chip_hide() : m.redacted_tag_chip_reveal()}
					onclick={() => toggle(span.value)}
				>
					<span class="value">{open ? span.value : '••••••'}</span>
				</Button>
			{/each}
		{/if}
		{#if unclosed}
			<Text size="xs" tone="warn" block data-testid="redacted-unclosed" role="status">
				{m.redacted_tag_unclosed()}
			</Text>
		{/if}
	</div>
{/if}

<style>
	.redacted {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-1);
	}
	.value {
		font-family: var(--font-mono);
		max-width: 24ch;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
</style>
