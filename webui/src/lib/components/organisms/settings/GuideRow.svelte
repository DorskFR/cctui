<script lang="ts">
	import { Badge, Button, Progress, Text } from '@dorsk/tsumikit';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import type { GuideStatus, GuideView } from '$lib/guides';
	import { m } from '$lib/paraglide/messages';

	let {
		guide,
		busy = false,
		hint,
		onlaunch
	}: {
		guide: GuideView;
		busy?: boolean;
		/** Live instance state the guide needs, stated before it is started. */
		hint?: string;
		onlaunch: (guide: GuideView) => void;
	} = $props();

	const STATUS_TONE = { done: 'ok', 'in-progress': 'warn', 'not-started': 'muted' } as const;

	function statusLabel(s: GuideStatus): string {
		if (s === 'done') return m.settings_guides_status_done();
		if (s === 'in-progress') return m.settings_guides_status_in_progress();
		return m.settings_guides_status_not_started();
	}

	const help = $derived(
		guide.locked
			? m.settings_guides_locked_by({ guides: guide.lockedBy.join(', ') })
			: hint
				? [guide.description, hint].filter(Boolean).join(' ')
				: guide.description
	);
	const stepLabel = $derived(
		guide.step === null
			? ''
			: guide.step.total > 0
				? m.settings_guides_step({ index: guide.step.index + 1, total: guide.step.total })
				: m.settings_guides_step_unknown({ index: guide.step.index + 1 })
	);
	const actionLabel = $derived(
		guide.status === 'in-progress'
			? m.settings_guides_resume()
			: guide.status === 'done'
				? m.settings_guides_replay()
				: m.settings_guides_start()
	);
</script>

<SettingRow label={guide.title} {help} disabled={guide.locked} selfLabelled>
	<span class="cell">
		<span class="chips">
			{#if guide.locked}
				<Badge tone="muted" size="sm" border>{m.settings_guides_locked()}</Badge>
			{:else}
				<Badge tone={STATUS_TONE[guide.status]} size="sm" border>{statusLabel(guide.status)}</Badge>
			{/if}
			<Badge tone={guide.status === 'done' ? 'accent' : 'neutral'} size="sm" border>
				{m.settings_guides_xp({ xp: guide.xp })}
			</Badge>
			<Button
				size="sm"
				loading={busy}
				disabled={guide.locked}
				onclick={() => onlaunch(guide)}
			>
				{actionLabel}
			</Button>
		</span>
		{#if guide.step && !guide.locked}
			<span class="step">
				<Progress
					size="sm"
					tone="warn"
					block
					value={guide.step.index}
					max={guide.step.total || guide.step.index + 1}
					label={stepLabel}
				/>
				<Text size="xs" tone="faint" as="span">{stepLabel}</Text>
			</span>
		{/if}
	</span>
</SettingRow>

<style>
	.cell {
		display: flex;
		flex-direction: column;
		align-items: flex-end;
		gap: var(--sp-1);
		width: 100%;
	}
	.chips {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		justify-content: flex-end;
	}
	.step {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		width: 100%;
	}
</style>
