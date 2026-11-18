<script lang="ts">
	import { Badge, Callout, Link, Text, Timestamp } from '@dorsk/tsumikit';
	import type { RedteamRun } from '@bindings/RedteamRun';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import { m } from '#lib/paraglide/messages.js';
	import { cellsEscapedFirst, escapeLabel, outcomeLabel, outcomeTone, probeSummary } from './redteam.logic';

	let { label, run }: { label: string; run: RedteamRun } = $props();

	const report = $derived(run.report);
	const cells = $derived(cellsEscapedFirst(report.cells));
</script>

<SettingRow {label} selfLabelled>
	{#snippet helpSlot()}
		<Timestamp value={report.finished_at * 1000} mode="relative" tone="inherit" />
		{' · '}{m.redteam_run_cells({ count: report.cells.length })}
		{#if report.model}{' · '}{report.model}{/if}
		{#if run.posted_by}{' · '}{m.redteam_run_posted_by({ user: run.posted_by })}{/if}
		{#if report.run_url}
			{' · '}<Link href={report.run_url} target="_blank" rel="noopener noreferrer">{m.redteam_run_link()}</Link>
		{/if}
	{/snippet}
	<Badge tone={outcomeTone(report.verdict)} size="sm">{outcomeLabel(report.verdict)}</Badge>
</SettingRow>

{#if run.escapes.length}
	<div class="escapes" data-redteam-escapes>
		<Callout tone="danger" icon="alert" title={m.redteam_escapes_title({ count: run.escapes.length })}>
			<ul>
				{#each run.escapes as e, i (i)}
					<li><Text size="sm" variant="code">{escapeLabel(e)}</Text></li>
				{/each}
			</ul>
		</Callout>
	</div>
{/if}

{#each report.reasons as reason, i (i)}
	<SettingRow label={m.redteam_run_reason()} help={reason} selfLabelled />
{/each}

{#each cells as c (c.cell)}
	<SettingRow label={c.cell} help={probeSummary(c) || c.reasons.join(' · ')} selfLabelled>
		<Badge tone={outcomeTone(c.verdict)} size="xs">{outcomeLabel(c.verdict)}</Badge>
	</SettingRow>
{/each}

<style>
	.escapes {
		padding: var(--sp-2) var(--sp-4);
	}
	ul {
		margin: 0;
		padding-left: var(--sp-4);
	}
</style>
