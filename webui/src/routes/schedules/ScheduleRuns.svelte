<script lang="ts">
	import { Badge, Link, Spinner, Text, Timestamp } from '@dorsk/tsumikit';
	import { errMessage } from '#lib/api.ts';
	import { usePresetRuns } from '#lib/queries/presets.ts';
	import { RUN_TONE, runRows, type RunState } from '#lib/schedules/runs.ts';
	import { sessionHref } from '#lib/sessionFailureToast.ts';
	import { m } from '#lib/paraglide/messages.js';

	let { presetId }: { presetId: string } = $props();

	const runs = usePresetRuns(
		() => presetId,
		() => true
	);
	const rows = $derived(runRows(runs.data ?? []));

	const label: Record<RunState, () => string> = {
		queued: m.schedules_run_queued,
		running: m.schedules_run_running,
		delivered: m.schedules_run_delivered,
		skipped: m.schedules_run_skipped,
		cancelled: m.schedules_run_cancelled,
		failed: m.schedules_run_failed
	};
</script>

{#if runs.isLoading}
	<Spinner />
{:else if runs.error}
	<Text size="sm" tone="danger">{errMessage(runs.error)}</Text>
{:else if rows.length === 0}
	<Text size="sm" tone="faint">{m.schedules_runs_empty()}</Text>
{:else}
	<ul class="runs" data-testid="schedule-runs">
		{#each rows as run (run.id)}
			<li class="run" data-state={run.state}>
				<Badge size="xs" tone={RUN_TONE[run.state]} border>{label[run.state]()}</Badge>
				<Timestamp value={run.at} mode="relative" tone="faint" size="xs" />
				{#if run.manual}
					<Badge size="xs" tone="muted">{m.schedules_run_manual()}</Badge>
				{/if}
				{#if run.sessionId}
					<Link href={sessionHref(run.sessionId)}>
						{m.schedules_run_session({ id: run.sessionId.slice(0, 8) })}
					</Link>
				{/if}
				{#if run.error}
					<Text size="xs" tone="danger">{run.error}</Text>
				{/if}
			</li>
		{/each}
	</ul>
{/if}

<style>
	.runs {
		display: flex;
		flex-direction: column;
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.run {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-1) 0;
		border-top: 1px solid var(--border);
	}
</style>
