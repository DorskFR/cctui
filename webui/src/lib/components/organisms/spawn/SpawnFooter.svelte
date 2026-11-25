<script lang="ts">
	import { Button, Callout, SplitButton } from '@dorsk/tsumikit';
	import { m } from '#lib/paraglide/messages.js';
	import SpawnAutoArchive from './SpawnAutoArchive.svelte';
	import type { SpawnForm } from './spawnForm.svelte';
	import type { SpawnSchedule } from './spawnSchedule.svelte';

	let { sf, schedule }: { sf: SpawnForm; schedule: SpawnSchedule } = $props();

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
</script>

{#if sf.spawnFailure}
	<Callout tone="danger" title={m.spawn_failure_inline()} style="flex-basis:100%">
		<pre class="spawn-failure-detail">{sf.spawnFailure}</pre>
	</Callout>
{/if}
{#if sf.target === 'machine' && !sf.noMachines}
	<SpawnAutoArchive bind:checked={sf.form.auto_archive} />
{/if}
<div class="foot-row">
	<Button style={FOOT_GROW} onclick={sf.clearForm}>{m.spawn_clear()}</Button>
	{#if sf.target === 'machine' && sf.canDraft}
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
	<!-- No machines or no scheduling: nothing to schedule, and only a plain
	     Button can carry the disabled reason as its own title. -->
	{#if sf.target === 'machine' && !sf.noMachines && sf.canSchedule}
		<SplitButton
			data-journey="submit"
			variant="primary"
			caret="half"
			style={FOOT_SPLIT_GROW}
			placement="top-end"
			label={m.spawn_schedule_menu()}
			items={schedule.items}
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
