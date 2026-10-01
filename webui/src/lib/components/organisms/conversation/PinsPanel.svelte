<script lang="ts">
	import { Button, IconButton, Text, Timestamp } from '@dorsk/tsumikit';
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
			<Button variant="ghost" size="sm" block onclick={() => onjump(pin.seq)}>
				<span class="jump-inner">
					<span class="role-dot" style={`--dot: var(--role-${pinRole(line)})`}></span>
					<Timestamp value={line?.ts ?? Date.parse(pin.created_at)} mode="time" tone="faint" size="xs" />
					<span class="excerpt">{pinExcerpt(line)}</span>
				</span>
			</Button>
			<IconButton
				inline
				glyphSize={14}
				icon="x"
				hoverDanger
				label={m.conversation_unpin_label()}
				title={m.conversation_unpin_title()}
				onclick={() => onunpin(pin.seq)}
			/>
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
		overflow-y: auto;
		overflow-x: hidden;
		padding: var(--sp-2);
	}
	.pin-row {
		display: grid;
		grid-template-columns: 1fr auto;
		align-items: center;
		gap: var(--sp-1);
	}
	.jump-inner {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		width: 100%;
		min-width: 0;
		color: var(--text);
		font-size: var(--fs-xs);
		text-align: left;
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
</style>
