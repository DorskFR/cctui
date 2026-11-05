<script lang="ts">
	import { Text } from '@dorsk/tsumikit';
	import type { MachineResourcesRow } from '@bindings/MachineResourcesRow';
	import { hashHue } from '$lib/format';
	import { m } from '$lib/paraglide/messages';
	import {
		fmtBytes,
		isStale,
		machineLabel,
		pctOf,
		worstTone
	} from '$lib/components/molecules/resource-gauge.logic';

	let { rows, now }: { rows: MachineResourcesRow[]; now: number } = $props();

	const pctText = (v: number | null | undefined) => {
		const p = pctOf(v);
		return p === null ? '?' : `${p}%`;
	};
	const detail = (res: NonNullable<MachineResourcesRow['resources']>) =>
		[
			`${m.resource_gauge_mem()} ${fmtBytes(res.mem_used_bytes)} / ${fmtBytes(res.mem_total_bytes)}`,
			`${m.resource_gauge_disk()} ${fmtBytes(res.disk_used_bytes)} / ${fmtBytes(res.disk_total_bytes)}`
		].join('\n');
</script>

<ul class="list">
	{#each rows as r (r.machine_id)}
		{@const res = r.resources ?? null}
		{@const stale = !res || isStale(r.updated_at, now)}
		<li class="row" title={res ? detail(res) : undefined}>
			<span class="head">
				<span class="dot" data-tone={stale ? 'unknown' : worstTone(res)} aria-hidden="true"></span>
				<span class="name" style={`--hue: ${r.hue ?? hashHue(r.name)}`}>{machineLabel(r)}</span>
			</span>
			{#if !res}
				<Text size="sm" tone="faint">{m.resource_gauge_unknown()}</Text>
			{:else}
				<span class="stats">
					{m.resource_gauge_cpu()} <span class="num">{pctText(res.cpu_pct)}</span> ·
					{m.resource_gauge_mem()} <span class="num">{pctText(res.mem_pct)}</span> ·
					{m.resource_gauge_disk()} <span class="num">{pctText(res.disk_pct)}</span>
				</span>
				{#if stale}<Text size="xs" tone="warn">{m.resource_gauge_stale()}</Text>{/if}
			{/if}
		</li>
	{/each}
</ul>

<style>
	.list {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		margin: 0;
		padding: 0;
		list-style: none;
		text-align: start;
	}
	.row {
		display: flex;
		flex-wrap: wrap;
		align-items: baseline;
		justify-content: flex-start;
		gap: var(--sp-1) var(--sp-3);
	}
	.head {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.name {
		font-size: var(--fs-sm);
		font-weight: 600;
		color: hsl(var(--hue) 60% 60%);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.stats {
		font-size: var(--fs-sm);
		color: var(--text-muted);
		white-space: nowrap;
	}
	.num {
		font-family: var(--font-numeric, inherit);
		font-variant-numeric: tabular-nums;
		color: var(--text);
	}
	.dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		flex: none;
		align-self: center;
		background: var(--ok);
	}
	.dot[data-tone='warn'] {
		background: var(--warn);
	}
	.dot[data-tone='danger'] {
		background: var(--danger);
	}
	.dot[data-tone='unknown'] {
		background: var(--dot-dead);
	}
</style>
