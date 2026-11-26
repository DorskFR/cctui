<script lang="ts">
	import { now as clockNow } from '#lib/clock.svelte.ts';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { Popover, Text } from '@dorsk/tsumikit';
	import { qk, useMachineResources } from '#lib/queries/index.ts';
	import type { MachineResourcesRow } from '@bindings/MachineResourcesRow';
	import { settings } from '#lib/settings.svelte.ts';
	import { ws } from '#lib/ws.svelte.ts';
	import { hashHue } from '#lib/format.ts';
	import { m } from '#lib/paraglide/messages.js';
	import UsageMeter from './UsageMeter.svelte';
	import {
		fmtBytes,
		isStale,
		machineLabel,
		patchRow,
		pctOf,
		pinnedRows,
		resourceTone,
		worstTone
	} from './resource-gauge.logic';

	const pinned = $derived(settings.monitoredMachines);
	const q = useMachineResources(() => pinned.length > 0);
	const rows = $derived(pinnedRows(q.data, pinned));

	const qc = useQueryClient();
	$effect(() =>
		ws.onMachineResources((ev) => {
			qc.setQueryData<MachineResourcesRow[]>(qk.machineResources, (old) =>
				patchRow(old, ev.machine_id, ev.resources, new Date().toISOString())
			);
		})
	);

	const now = $derived(clockNow(30_000));

	const hueOf = (r: MachineResourcesRow) => r.hue ?? hashHue(r.name);
	const pctText = (v: number | null | undefined) => {
		const p = pctOf(v);
		return p === null ? '?' : `${p}%`;
	};
	function titleOf(r: MachineResourcesRow): string {
		const res = r.resources;
		const stale = isStale(r.updated_at, now);
		const lines = [machineLabel(r)];
		if (!res) {
			lines.push(m.resource_gauge_unknown());
		} else {
			lines.push(
				`${m.resource_gauge_cpu()} ${pctText(res.cpu_pct)}` +
					(res.load1 !== null && res.load1 !== undefined ? ` (load ${res.load1.toFixed(2)})` : ''),
				`${m.resource_gauge_mem()} ${pctText(res.mem_pct)} (${fmtBytes(res.mem_used_bytes)} / ${fmtBytes(res.mem_total_bytes)})`,
				`${m.resource_gauge_disk()} ${pctText(res.disk_pct)} (${fmtBytes(res.disk_used_bytes)} / ${fmtBytes(res.disk_total_bytes)}, ${res.disk_path})`
			);
			if (stale) lines.push(m.resource_gauge_stale());
		}
		return lines.join('\n');
	}
	function ariaOf(r: MachineResourcesRow): string {
		return m.resource_gauge_aria({
			machine: machineLabel(r),
			cpu: pctText(r.resources?.cpu_pct),
			mem: pctText(r.resources?.mem_pct),
			disk: pctText(r.resources?.disk_pct)
		});
	}
</script>

{#snippet bar(v: number | null | undefined, stale: boolean)}
	{@const pct = pctOf(v)}
	<span class="track" data-tone={stale ? 'unknown' : resourceTone(v)}>
		{#if pct !== null && !stale}<span class="fill" style={`width: ${pct}%`}></span>{/if}
	</span>
{/snippet}

{#snippet stat(label: string, v: number | null | undefined, stale: boolean, detail: string)}
	<span class="stat">
		<Text size="sm" tone="muted">{label}</Text>
		<UsageMeter pct={pctOf(v)} tone={stale ? 'unknown' : resourceTone(v)} width={120} />
		<span class="stat-pct"><Text size="sm" numeric weight="semibold">{pctText(v)}</Text></span>
		<Text size="xs" tone="faint" numeric nowrap>{detail}</Text>
	</span>
{/snippet}

{#if rows.length > 0}
	<span class="strip">
		{#each rows as r (r.machine_id)}
			{@const res = r.resources ?? null}
			{@const stale = !res || isStale(r.updated_at, now)}
			{@const worst = stale ? 'unknown' : worstTone(res)}
			<Popover label={ariaOf(r)} placement="bottom-end" bare hitArea="compact">
				{#snippet trigger()}
					<span class="cell" title={titleOf(r)}>
						<span class="name" style={`--hue: ${hueOf(r)}`}>{machineLabel(r)}</span>
						<span class="bars">
							{@render bar(res?.cpu_pct, stale)}
							{@render bar(res?.mem_pct, stale)}
							{@render bar(res?.disk_pct, stale)}
						</span>
						{#if stale}<span class="unknown" aria-hidden="true">?</span>{/if}
						<span class="dot" data-tone={worst} aria-hidden="true"></span>
					</span>
				{/snippet}
				<div class="panel">
					<span class="panel-name" style={`--hue: ${hueOf(r)}`}
						><Text size="sm" weight="semibold" tone="inherit">{machineLabel(r)}</Text></span
					>
					{#if !res}
						<Text size="sm" tone="faint">{m.resource_gauge_unknown()}</Text>
					{:else}
						{@const load =
							typeof res.load1 === 'number' ? m.resource_gauge_load({ load: res.load1.toFixed(2) }) : ''}
						<div class="stats">
							{@render stat(m.resource_gauge_cpu(), res.cpu_pct, stale, load)}
							{@render stat(
								m.resource_gauge_mem(),
								res.mem_pct,
								stale,
								`${fmtBytes(res.mem_used_bytes)} / ${fmtBytes(res.mem_total_bytes)}`
							)}
							{@render stat(
								m.resource_gauge_disk(),
								res.disk_pct,
								stale,
								`${fmtBytes(res.disk_used_bytes)} / ${fmtBytes(res.disk_total_bytes)}`
							)}
						</div>
						{#if res.disk_path}<Text size="xs" tone="faint" mono truncate>{res.disk_path}</Text>{/if}
						{#if stale}<Text size="xs" tone="danger">{m.resource_gauge_stale()}</Text>{/if}
					{/if}
				</div>
			</Popover>
		{/each}
	</span>
{/if}

<style>
	/* Lives in the px-pinned header: every length is px, never rem. */
	.strip {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		flex: none;
	}
	.cell {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		height: 24px;
		padding: 0 4px;
		border-radius: 4px;
		background: var(--bg-elevated-2);
	}
	.name {
		font-size: 10px;
		line-height: 1;
		font-weight: 600;
		max-width: 64px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: hsl(var(--hue) 60% 60%);
	}
	.bars {
		display: inline-flex;
		flex-direction: column;
		gap: 2px;
	}
	.track {
		display: block;
		width: 22px;
		height: 3px;
		border-radius: 2px;
		background: var(--border);
		overflow: hidden;
	}
	.track[data-tone='unknown'] {
		background: repeating-linear-gradient(45deg, var(--border-strong) 0 2px, transparent 2px 4px);
	}
	.fill {
		display: block;
		height: 100%;
		border-radius: 2px;
	}
	.track[data-tone='ok'] .fill {
		background: var(--ok);
	}
	.track[data-tone='warn'] .fill {
		background: var(--warn);
	}
	.track[data-tone='danger'] .fill {
		background: var(--danger);
	}
	.unknown {
		font-size: 10px;
		line-height: 1;
		color: var(--text-faint);
	}
	/* The dot is the narrow-screen face of the cell: the machine's worst bar. */
	.dot {
		display: none;
		width: 8px;
		height: 8px;
		border-radius: 50%;
		flex: none;
	}
	.dot[data-tone='ok'] {
		background: var(--ok);
	}
	.dot[data-tone='warn'] {
		background: var(--warn);
	}
	.dot[data-tone='danger'] {
		background: var(--danger);
		box-shadow: 0 0 6px var(--danger);
	}
	.dot[data-tone='unknown'] {
		background: var(--dot-dead);
	}
	@media (max-width: 1023px) {
		.name,
		.bars,
		.unknown {
			display: none;
		}
		.dot {
			display: inline-block;
		}
		.cell {
			padding: 0 3px;
		}
	}
	.panel {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		min-width: 18rem;
		max-width: 100%;
	}
	.panel-name {
		color: hsl(var(--hue) 60% 60%);
	}
	.stats {
		display: grid;
		grid-template-columns: auto auto auto 1fr;
		gap: var(--sp-2) var(--sp-3);
		align-items: center;
	}
	.stat {
		display: contents;
	}
	.stat-pct {
		text-align: right;
		min-width: 4ch;
	}
</style>
