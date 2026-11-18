<script lang="ts">
	import type { LaunchPreset } from '@bindings/LaunchPreset';
	import { Badge, Button, Card, Disclosure, IconButton, Switch, Text, Timestamp } from '@dorsk/tsumikit';
	import { getLocale } from '#lib/paraglide/runtime.js';
	import { m } from '#lib/paraglide/messages.js';
	import { cronWords } from '#lib/schedules/cronWords.ts';
	import ScheduleRuns from './ScheduleRuns.svelte';

	let {
		preset,
		busy = false,
		ontoggle,
		onrun,
		onedit,
		ondelete
	}: {
		preset: LaunchPreset;
		busy?: boolean;
		ontoggle: (enabled: boolean) => void;
		onrun: () => void;
		onedit: () => void;
		ondelete: () => void;
	} = $props();

	let runsOpen = $state(false);
	const cron = $derived(preset.cron ?? '');
</script>

<Card padding="sm" data-testid="schedule-row" data-enabled={preset.enabled}>
	<div class="head">
		<div class="what">
			<Text weight="bold">{preset.name}</Text>
			<Text size="sm" tone="muted">
				{cronWords(cron, getLocale())}
				<Text as="span" size="xs" tone="faint">· {preset.timezone}</Text>
			</Text>
		</div>
		<div class="actions">
			<Switch
				size="sm"
				bind:checked={() => preset.enabled, (v: boolean) => ontoggle(v)}
				disabled={busy}
				label={m.schedules_enabled_aria({ name: preset.name })}
			/>
			<Button size="sm" disabled={busy} onclick={onrun}>{m.schedules_run_now()}</Button>
			<IconButton
				icon="edit"
				box="sm"
				variant="ghost"
				label={m.schedules_edit_aria({ name: preset.name })}
				onclick={onedit}
			/>
			<IconButton
				icon="trash"
				box="sm"
				variant="ghost"
				hoverDanger
				disabled={busy}
				label={m.schedules_delete_aria({ name: preset.name })}
				onclick={ondelete}
			/>
		</div>
	</div>
	<div class="meta">
		<Text size="xs" variant="code" title={m.schedules_cron_label()}>{cron}</Text>
		{#if !preset.enabled}
			<Badge size="xs" tone="muted" border>{m.schedules_paused()}</Badge>
		{:else if preset.next_fire_at}
			<Text size="xs" tone="faint">
				{m.schedules_next_fire()}
				<Timestamp value={Date.parse(preset.next_fire_at)} mode="relative" size="xs" />
			</Text>
		{/if}
		{#if preset.last_fire_at}
			<Text size="xs" tone="faint">
				{m.schedules_last_fire()}
				<Timestamp value={Date.parse(preset.last_fire_at)} mode="relative" size="xs" />
			</Text>
		{/if}
	</div>
	<Disclosure bind:open={runsOpen} size="compact">
		{#snippet header()}
			<Text size="xs" tone="faint" weight="bold">{m.schedules_runs_title()}</Text>
		{/snippet}
		{#if runsOpen}
			<ScheduleRuns presetId={preset.id} />
		{/if}
	</Disclosure>
</Card>

<style>
	.head {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
	}
	.what {
		display: flex;
		flex-direction: column;
		flex: 1 1 14rem;
		min-width: 0;
	}
	.actions {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		margin-left: auto;
	}
	.meta {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
		margin: var(--sp-2) 0;
	}
</style>
