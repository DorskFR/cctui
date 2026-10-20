<script lang="ts">
	// The session's workflow-guard step as its pod reports it, with a request
	// to move. The pod's guard decides; this only shows the mirror and asks.
	import type { GuardStepState } from '@bindings/GuardStepState';
	import { Badge, IconButton, Select } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		step,
		disabled = false,
		ontransition
	}: {
		step: GuardStepState;
		/** No control: an archived session, or a request already in flight. */
		disabled?: boolean;
		ontransition?: (step: string) => void | Promise<void>;
	} = $props();

	const exited = $derived(step.step < 0);
	const targets = $derived(exited ? [] : [...step.transitions.map(String), 'exit']);
	const label = $derived.by(() => {
		if (exited) return m.guard_step_exited();
		const title = step.title.trim();
		return title && title !== 'unknown'
			? m.guard_step_badge_titled({ step: step.step, title })
			: m.guard_step_badge({ step: step.step });
	});
	const hint = $derived(
		m.guard_step_title({ allowed: step.allowed || '—', disallowed: step.disallowed || '—' })
	);

	let target = $state('');
	$effect(() => {
		if (!targets.includes(target)) target = targets[0] ?? '';
	});

	function go() {
		if (!target || !ontransition) return;
		void ontransition(target);
	}
</script>

<span class="guard" data-journey="head-guard" title={hint}>
	<Badge tone={exited ? 'muted' : 'info'} mono style="display:inline-flex;align-items:center;gap:var(--sp-1);min-width:0;max-width:100%">
		<span class="label">⛨ {label}</span>
		{#if !exited && ontransition}
			<Select
				variant="embedded"
				width="auto"
				size="sm"
				chevron={false}
				bind:value={target}
				{disabled}
				aria-label={m.guard_step_target_aria()}
			>
				{#each targets as t (t)}
					<option value={t}>{t === 'exit' ? m.guard_step_exit_option() : m.guard_step_step_option({ step: t })}</option>
				{/each}
			</Select>
			<IconButton chip variant="default" icon="arrow-right" label={m.guard_step_go()} title={m.guard_step_go_title()} {disabled} onclick={go} />
		{/if}
	</Badge>
</span>

<style>
	.guard {
		display: inline-flex;
		flex: 0 1 auto;
		min-width: 0;
	}
	.label {
		min-width: 0;
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
</style>
