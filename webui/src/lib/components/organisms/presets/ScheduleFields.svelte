<script lang="ts">
	import { Field, Input, SegmentedControl, Select, Switch, Text, Toggle } from '@dorsk/tsumikit';
	import type { SelectOption } from '@dorsk/tsumikit';
	import type { Overlap } from '@bindings/Overlap';
	import { getLocale } from '#lib/paraglide/runtime.js';
	import { m } from '#lib/paraglide/messages.js';
	import { REPEAT_KINDS, cronFromRepeat, cronProblem, normalizeWeekdays, type RepeatKind } from '#lib/schedules/cron.ts';
	import { cronProblemText, cronWords, weekdayName } from '#lib/schedules/cronWords.ts';
	import {
		supportedTimezones,
		timezoneOptions,
		type PresetForm,
		type PresetFormProblems
	} from '#lib/schedules/presetForm.ts';

	let {
		form = $bindable(),
		problems,
		showErrors = false
	}: {
		form: PresetForm;
		problems: PresetFormProblems;
		showErrors?: boolean;
	} = $props();

	const uid = $props.id();
	const WEEK = [1, 2, 3, 4, 5, 6, 0];

	const kindLabel: Record<RepeatKind, () => string> = {
		hourly: m.schedules_repeat_hourly,
		daily: m.schedules_repeat_daily,
		weekdays: m.schedules_repeat_weekdays,
		weekly: m.schedules_repeat_weekly,
		custom: m.schedules_repeat_custom
	};
	const kinds = $derived(REPEAT_KINDS.map((value) => ({ value, label: kindLabel[value]() })));
	const timed = $derived(form.repeat.kind !== 'hourly' && form.repeat.kind !== 'custom');
	const cron = $derived(cronFromRepeat(form.repeat));
	const customProblem = $derived(form.repeat.custom.trim() ? cronProblem(form.repeat.custom) : null);
	const cronError = $derived(
		customProblem
			? cronProblemText(customProblem)
			: showErrors && problems.cron
				? cronProblemText(problems.cron)
				: undefined
	);
	const zones = $derived<SelectOption[]>(
		timezoneOptions(supportedTimezones(), form.timezone).map((z) => ({ value: z, label: z }))
	);
	const overlaps = $derived<SelectOption[]>([
		{ value: 'skip', label: m.schedules_overlap_skip() },
		{ value: 'allow', label: m.schedules_overlap_allow() }
	]);

	function pickKind(kind: string) {
		if (kind === 'custom' && form.repeat.kind !== 'custom') form.repeat.custom = cron;
		form.repeat.kind = kind as RepeatKind;
	}
	function toggleDay(day: number) {
		const days = form.repeat.weekdays;
		form.repeat.weekdays = normalizeWeekdays(
			days.includes(day) ? days.filter((d) => d !== day) : [...days, day]
		);
	}
</script>

<div class="schedule" data-testid="schedule-fields">
	<Field label={m.schedules_repeat_label()}>
		<SegmentedControl
			options={kinds}
			value={form.repeat.kind}
			onchange={pickKind}
			label={m.schedules_repeat_label()}
			wrap
		/>
	</Field>

	{#if timed}
		<Field
			label={m.schedules_time_label()}
			for="{uid}-time"
			error={showErrors && problems.time ? m.schedules_time_invalid() : undefined}
		>
			<Input id="{uid}-time" type="time" width="9rem" bind:value={form.repeat.time} />
		</Field>
	{/if}

	{#if form.repeat.kind === 'weekly'}
		<Field
			label={m.schedules_weekdays_label()}
			error={showErrors && problems.weekdays ? m.schedules_weekdays_required() : undefined}
		>
			<div class="days" role="group" aria-label={m.schedules_weekdays_label()}>
				{#each WEEK as day (day)}
					<Toggle
						size="sm"
						pressed={form.repeat.weekdays.includes(day)}
						aria-label={weekdayName(day, getLocale())}
						onclick={() => toggleDay(day)}
					>
						{weekdayName(day, getLocale(), 'short')}
					</Toggle>
				{/each}
			</div>
		</Field>
	{/if}

	{#if form.repeat.kind === 'custom'}
		<Field label={m.schedules_cron_label()} for="{uid}-cron" hint={m.schedules_cron_hint()} error={cronError}>
			<Input
				id="{uid}-cron"
				mono
				placeholder="0 9 * * 1-5"
				invalid={!!cronError}
				bind:value={form.repeat.custom}
			/>
		</Field>
	{/if}

	{#if !cronProblem(cron)}
		<Text size="xs" tone="faint" data-testid="schedule-words">
			{cronWords(cron, getLocale())} · <Text as="span" size="xs" variant="code">{cron}</Text>
		</Text>
	{/if}

	<Field label={m.schedules_timezone_label()} for="{uid}-tz" hint={m.schedules_timezone_hint()}>
		<Select id="{uid}-tz" options={zones} bind:value={form.timezone} />
	</Field>

	<Field label={m.schedules_overlap_label()} for="{uid}-overlap" hint={m.schedules_overlap_hint()}>
		<Select
			id="{uid}-overlap"
			options={overlaps}
			bind:value={() => form.overlap, (v: string) => (form.overlap = v as Overlap)}
		/>
	</Field>

	<Switch
		id="{uid}-enabled"
		bind:checked={form.enabled}
		label={m.schedules_enabled_label()}
		labelVisible
		size="sm"
	/>
</div>

<style>
	.schedule {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.days {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-1);
	}
</style>
