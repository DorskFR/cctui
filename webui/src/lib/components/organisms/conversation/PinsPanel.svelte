<script lang="ts">
	import { Text, Timestamp } from '@dorsk/tsumikit';
	import type { MessagePin } from '@bindings/MessagePin';
	import type { Line } from './types';
	import { pinExcerpt, pinRole } from './pins';
	import { m } from '$lib/paraglide/messages';

	let {
		pins,
		lines,
		onjump,
		onunpin
	}: {
		pins: MessagePin[];
		lines: Line[];
		onjump: (seq: number) => void;
		onunpin: (seq: number) => void;
	} = $props();

	const bySeq = $derived(new Map(lines.filter((l) => l.seq !== undefined).map((l) => [l.seq, l])));
	const rows = $derived(
		[...pins]
			.sort((a, b) => a.seq - b.seq)
			.map((p) => ({ pin: p, line: bySeq.get(p.seq) }))
	);
</script>

<div class="pins">
	{#if rows.length === 0}
		<Text tone="faint" size="xs">{m.conversation_pins_empty()}</Text>
	{/if}
	{#each rows as { pin, line } (pin.seq)}
		<div class="pin-row">
			<button type="button" class="pin-jump" onclick={() => onjump(pin.seq)}>
				<span class="role-dot" style={`--dot: var(--role-${pinRole(line)})`}></span>
				<Timestamp value={line?.ts ?? Date.parse(pin.created_at)} mode="time" tone="faint" size="xs" />
				<span class="excerpt">{pinExcerpt(line)}</span>
			</button>
			<button
				type="button"
				class="pin-remove"
				aria-label={m.conversation_unpin_label()}
				title={m.conversation_unpin_title()}
				onclick={() => onunpin(pin.seq)}>✕</button
			>
		</div>
	{/each}
</div>

<style>
	.pins {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 16rem;
		max-width: 26rem;
		max-height: 50vh;
		overflow: auto;
		padding: var(--sp-2);
	}
	.pin-row {
		display: flex;
		align-items: center;
		gap: var(--sp-1);
	}
	.pin-jump {
		flex: 1;
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
		padding: var(--sp-1) var(--sp-2);
		background: none;
		border: none;
		border-radius: var(--r-2, 6px);
		color: var(--text);
		font-size: var(--fs-xs);
		text-align: left;
		cursor: pointer;
	}
	.pin-jump:hover {
		background: var(--bg-elevated-2);
	}
	.role-dot {
		flex: none;
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: var(--dot, var(--text-muted));
	}
	.excerpt {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--text-muted);
	}
	.pin-remove {
		flex: none;
		padding: 0 var(--sp-1);
		background: none;
		border: none;
		color: var(--text-faint);
		cursor: pointer;
	}
	.pin-remove:hover {
		color: var(--danger);
	}
</style>
