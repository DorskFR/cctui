<script lang="ts">
	import { Popover, Text } from '@dorsk/tsumikit';
	import { now as clockNow } from '#lib/clock.svelte.ts';
	import { useMachineResources } from '#lib/queries/index.ts';
	import { settings } from '#lib/settings.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';
	import UsageDetail from '#lib/components/molecules/UsageDetail.svelte';
	import { readingText, useUsageGauge } from '#lib/components/molecules/usageGauge.svelte.ts';
	import {
		isStale,
		machineLabel,
		pctOf,
		pinnedRows,
		worstTone,
		type ResourceTone
	} from '#lib/components/molecules/resource-gauge.logic.ts';

	const gauge = useUsageGauge();
	const pinned = $derived(settings.monitoredMachines);
	const machines = useMachineResources(() => pinned.length > 0);
	const rows = $derived(pinnedRows(machines.data, pinned));
	const now = $derived(clockNow(30_000));

	const RANK: Record<ResourceTone, number> = { unknown: 0, ok: 1, warn: 2, danger: 3 };
	const machineTone = (r: (typeof rows)[number]): ResourceTone =>
		!r.resources || isStale(r.updated_at, now) ? 'unknown' : worstTone(r.resources);
	const tones = $derived<ResourceTone[]>([...rows.map(machineTone), gauge.worst?.tone ?? 'unknown']);
	const tone = $derived(tones.reduce((a, b) => (RANK[b] > RANK[a] ? b : a), 'unknown'));
	const pct = (v: number | null | undefined) => {
		const p = pctOf(v);
		return p === null ? '?' : `${p}%`;
	};
	const label = $derived(
		gauge.worst ? m.header_status_label_usage({ usage: readingText(gauge.worst) }) : m.header_status_label()
	);
	let open = $state(false);
	const PANEL_STYLE = 'max-width:min(30rem, calc(100vw - 2 * var(--sp-3)))';
</script>

{#if gauge.worst || rows.length > 0}
	<Popover bind:open {label} placement="bottom-end" bare hitArea="compact" panelStyle={PANEL_STYLE}>
		{#snippet trigger()}
			<span class="status" data-tone={tone}>
				<span class="dot" aria-hidden="true"></span>
				{#if gauge.worst}<Text size="xs" numeric nowrap tone="inherit">{readingText(gauge.worst)}</Text>{/if}
			</span>
		{/snippet}
		<div class="panel">
			{#if gauge.rows.length > 0}
				<section>
					<Text size="xs" tone="muted" weight="semibold">{m.header_status_usage()}</Text>
					<UsageDetail rows={gauge.rows} onnavigate={() => (open = false)} />
				</section>
			{/if}
			{#if rows.length > 0}
				<section>
					<Text size="xs" tone="muted" weight="semibold">{m.header_status_machines()}</Text>
					<ul class="machines">
						{#each rows as r (r.machine_id)}
							<li data-tone={machineTone(r)}>
								<span class="name"><Text size="sm" truncate>{machineLabel(r)}</Text></span>
								<span class="stats">
									{#if r.resources && !isStale(r.updated_at, now)}
										<Text size="sm" numeric nowrap tone="muted"
											>{m.resource_gauge_cpu()} {pct(r.resources.cpu_pct)} · {m.resource_gauge_mem()}
											{pct(r.resources.mem_pct)} · {m.resource_gauge_disk()} {pct(r.resources.disk_pct)}</Text
										>
									{:else}
										<Text size="sm" tone="faint">{m.resource_gauge_unknown()}</Text>
									{/if}
								</span>
							</li>
						{/each}
					</ul>
				</section>
			{/if}
		</div>
	</Popover>
{/if}

<style>
	.status {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: 6px;
		min-width: 44px;
		min-height: 44px;
		padding: 0 8px;
		border-radius: 8px;
		color: var(--text-muted);
	}
	.status[data-tone='warn'] {
		color: var(--warn);
	}
	.status[data-tone='danger'] {
		color: var(--danger);
	}
	.dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		flex: none;
		background: var(--dot-dead);
	}
	.status[data-tone='ok'] .dot {
		background: var(--ok);
	}
	.status[data-tone='warn'] .dot {
		background: var(--warn);
	}
	.status[data-tone='danger'] .dot {
		background: var(--danger);
	}
	.panel {
		display: flex;
		flex-direction: column;
		gap: var(--sp-4);
		width: max-content;
		min-width: min(20rem, 100%);
		max-width: 100%;
		padding: var(--sp-2) var(--sp-3);
		box-sizing: border-box;
		max-height: 75dvh;
		overflow: hidden auto;
		overscroll-behavior: contain;
	}
	section {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.machines {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.machines li {
		display: flex;
		flex-wrap: wrap;
		column-gap: var(--sp-3);
		min-width: 0;
	}
	.name {
		display: flex;
		flex: 0 1 auto;
		min-width: 0;
	}
	.stats {
		display: flex;
		flex: none;
		margin-inline-start: auto;
	}
</style>
