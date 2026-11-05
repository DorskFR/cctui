<script lang="ts">
	import { Button, Callout, SplitButton } from '@dorsk/tsumikit';
	import ScheduleCustomModal from '../conversation/ScheduleCustomModal.svelte';
	import { scheduleMenuItems } from '../conversation/scheduleMenu';
	import { parseCustom, toLocalInput } from '../conversation/scheduleTimes';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';
	import type { SpawnForm } from './spawnForm.svelte';

	let { sf }: { sf: SpawnForm } = $props();

	// The kit's Button/SplitButton take no flex prop whose basis is the content
	// (`grow` is `1 1 0`, i.e. equal columns), so the footer buttons carry theirs
	// inline. The primary's basis is the width below which it would be squeezed:
	// past it the row wraps and it spans the whole footer.
	const FOOT_GROW = 'flex: 1 1 auto';
	const FOOT_PRIMARY_GROW = 'flex: 1 1 11rem';
	// SplitButton packs its two segments at the start of its box, so a stretched
	// root would leave slack after the caret; a grid root hands the slack to the
	// main segment and keeps the caret glued to it.
	const FOOT_SPLIT_GROW = `${FOOT_PRIMARY_GROW}; display: grid; grid-template-columns: 1fr auto`;

	let customOpen = $state(false);
	let customValue = $state('');
	// The presets are relative to now, so a form left open past a cutoff
	// (e.g. "later today") must not keep offering a time that has passed.
	let now = $state(Date.now());
	$effect(() => {
		const t = setInterval(() => (now = Date.now()), 60_000);
		return () => clearInterval(t);
	});
	const scheduleItems = $derived(
		scheduleMenuItems({
			now,
			canSchedule: sf.draftValid && !sf.busy,
			onpreset: (at) => void sf.scheduleLaunch(at),
			oncustom: () => {
				customValue = toLocalInput(new Date(Date.now() + 3_600_000));
				customOpen = true;
			}
		})
	);

	function scheduleCustom() {
		const at = parseCustom(customValue, new Date());
		if (!at) {
			toasts.error(m.composer_schedule_custom_invalid());
			return;
		}
		customOpen = false;
		void sf.scheduleLaunch(at);
	}
</script>

{#if sf.spawnFailure}
	<Callout tone="danger" title={m.spawn_failure_inline()} style="flex-basis:100%">
		<pre class="spawn-failure-detail">{sf.spawnFailure}</pre>
	</Callout>
{/if}
<div class="foot-row">
	<Button style={FOOT_GROW} onclick={sf.clearForm}>{m.spawn_clear()}</Button>
	{#if sf.target === 'machine'}
		<Button
			data-journey="draft"
			style={FOOT_GROW}
			disabled={sf.busy || !sf.draftValid}
			title={sf.disabledReason}
			onclick={sf.submitDraft}
		>
			{m.spawn_draft()}
		</Button>
	{/if}
	<!-- No machines: nothing to schedule either, and only a plain Button can
	     carry the disabled reason as its own title. -->
	{#if sf.target === 'machine' && !sf.noMachines}
		<SplitButton
			data-journey="submit"
			variant="primary"
			caret="half"
			style={FOOT_SPLIT_GROW}
			placement="top-end"
			label={m.spawn_schedule_menu()}
			items={scheduleItems}
			loading={sf.busy}
			disabled={sf.busy || !sf.valid}
			menuDisabled={!sf.draftValid}
			title={sf.disabledReason}
			onclick={sf.submit}
		>
			{sf.spawnLabel}
		</SplitButton>
	{:else}
		<Button
			data-journey="submit"
			variant="primary"
			style={FOOT_PRIMARY_GROW}
			loading={sf.busy}
			disabled={sf.busy || !sf.valid}
			title={sf.disabledReason}
			onclick={sf.submit}
		>
			{sf.spawnLabel}
		</Button>
	{/if}
</div>

{#if customOpen}
	<ScheduleCustomModal
		now={Date.now()}
		bind:value={customValue}
		onconfirm={scheduleCustom}
		onclose={() => (customOpen = false)}
	/>
{/if}

<style>
	.spawn-failure-detail {
		margin: var(--sp-1) 0 0;
		max-height: 8rem;
		overflow-y: auto;
		overflow-x: hidden;
		white-space: pre-wrap;
		font-family: var(--font-mono);
		font-size: var(--text-xs);
	}
	.foot-row {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-2);
		flex: 1 1 auto;
		width: 100%;
	}
</style>
