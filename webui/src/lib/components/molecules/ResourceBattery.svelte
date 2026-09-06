<script lang="ts">
	// Header resource strip: one cell per machine ticked in Settings › Resource
	// monitoring, three battery-style bars (CPU, memory, disk) toned green /
	// orange / red at a glance. On a narrow screen each machine collapses to a
	// single dot in its worst tone. A machine whose daemon never reported (too
	// old, non-Linux) shows hatched bars and "?", never a misleading 0 %.
	import { useQueryClient } from '@tanstack/svelte-query';
	import { Popover, Text } from '@dorsk/tsumikit';
	import { qk, useMachineResources } from '$lib/queries';
	import type { MachineResourcesRow } from '@bindings/MachineResourcesRow';
	import { settings } from '$lib/settings.svelte';
	import { ws } from '$lib/ws.svelte';
	import { hashHue } from '$lib/format';
	import { m } from '$lib/paraglide/messages';
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

	// Live: every daemon heartbeat lands here; patch the cache in place so the
	// bars move without waiting for the 30s poll.
	const qc = useQueryClient();
	$effect(() =>
		ws.onMachineResources((ev) => {
			qc.setQueryData<MachineResourcesRow[]>(qk.machineResources, (old) =>
				patchRow(old, ev.machine_id, ev.resources, new Date().toISOString())
			);
		})
	);

	let now = $state(Date.now());
	$effect(() => {
		const id = setInterval(() => (now = Date.now()), 30_000);
		return () => clearInterval(id);
	});

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
					<Text size="sm" weight="semibold">{machineLabel(r)}</Text>
					{#if !res}
						<Text size="sm" tone="faint">{m.resource_gauge_unknown()}</Text>
					{:else}
						<div class="grid">
							<Text size="sm" tone="faint">{m.resource_gauge_cpu()}</Text>
							<Text size="sm" variant="code">{pctText(res.cpu_pct)}</Text>
							<Text size="sm" tone="faint">{m.resource_gauge_mem()}</Text>
							<Text size="sm" variant="code"
								>{pctText(res.mem_pct)} · {fmtBytes(res.mem_used_bytes)} / {fmtBytes(
									res.mem_total_bytes
								)}</Text
							>
							<Text size="sm" tone="faint">{m.resource_gauge_disk()}</Text>
							<Text size="sm" variant="code"
								>{pctText(res.disk_pct)} · {fmtBytes(res.disk_used_bytes)} / {fmtBytes(
									res.disk_total_bytes
								)}</Text
							>
						</div>
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
		border: 1px solid var(--border);
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
		min-width: 14rem;
		max-width: 100%;
	}
	.grid {
		display: grid;
		grid-template-columns: auto 1fr;
		gap: var(--sp-1) var(--sp-3);
		align-items: baseline;
	}
</style>
