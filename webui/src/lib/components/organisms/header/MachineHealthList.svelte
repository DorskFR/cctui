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
</script>

<ul class="list">
	{#each rows as r (r.machine_id)}
		{@const res = r.resources ?? null}
		{@const stale = !res || isStale(r.updated_at, now)}
		<li class="row">
			<span class="head">
				<span class="dot" data-tone={stale ? 'unknown' : worstTone(res)} aria-hidden="true"></span>
				<span class="name" style={`--hue: ${r.hue ?? hashHue(r.name)}`}>{machineLabel(r)}</span>
			</span>
			{#if !res}
				<Text size="sm" tone="faint">{m.resource_gauge_unknown()}</Text>
			{:else}
				<span class="grid">
					<Text size="sm" tone="muted">{m.resource_gauge_cpu()}</Text>
					<span class="num">{pctText(res.cpu_pct)}</span>
					<Text size="sm" tone="muted">{m.resource_gauge_mem()}</Text>
					<span class="num"
						>{pctText(res.mem_pct)} · {fmtBytes(res.mem_used_bytes)} / {fmtBytes(res.mem_total_bytes)}</span
					>
					<Text size="sm" tone="muted">{m.resource_gauge_disk()}</Text>
					<span class="num"
						>{pctText(res.disk_pct)} · {fmtBytes(res.disk_used_bytes)} / {fmtBytes(res.disk_total_bytes)}</span
					>
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
		gap: var(--sp-3);
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.row {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	.head {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.name {
		font-size: var(--fs-sm);
		font-weight: 600;
		color: hsl(var(--hue) 60% 60%);
	}
	.grid {
		display: grid;
		grid-template-columns: auto 1fr;
		gap: var(--sp-1) var(--sp-3);
		align-items: baseline;
	}
	.num {
		font-family: var(--font-numeric, inherit);
		font-variant-numeric: tabular-nums;
		font-size: var(--fs-sm);
	}
	.dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		flex: none;
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
