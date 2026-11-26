<script lang="ts">
	import { Button, Callout, Icon, SplitButton } from '@dorsk/tsumikit';
	import { m } from '#lib/paraglide/messages.js';
	import type { SpawnForm } from './spawnForm.svelte';
	import type { SpawnSchedule } from './spawnSchedule.svelte';

	let { sf, schedule }: { sf: SpawnForm; schedule: SpawnSchedule } = $props();

	// The kit's buttons take no margin prop; the primary is pushed to the end of
	// the row, and to the end of its own line once the row wraps.
	const PRIMARY_END = 'margin-inline-start: auto';
	const CHORD = 'Control+Enter Meta+Enter';
</script>

<div class="spawn-foot">
	{#if sf.spawnFailure}
		<Callout tone="danger" title={m.spawn_failure_inline()}>
			<pre class="spawn-failure-detail">{sf.spawnFailure}</pre>
		</Callout>
	{/if}
	<div class="foot-row">
		<div class="foot-quiet">
			<Button variant="ghost" size="sm" onclick={sf.clearForm}>{m.spawn_clear()}</Button>
			{#if sf.target === 'machine' && sf.canDraft}
				<Button
					data-journey="draft"
					variant="ghost"
					size="sm"
					disabled={sf.busy || !sf.draftValid}
					title={sf.disabledReason}
					onclick={sf.submitDraft}
				>
					{m.spawn_draft()}
				</Button>
			{/if}
		</div>
		<!-- With no machines only a plain Button can carry the disabled reason as its own title. -->
		{#if sf.target === 'machine' && !sf.noMachines}
			<SplitButton
				data-journey="submit"
				variant="primary"
				caret="half"
				style={PRIMARY_END}
				placement="top-end"
				label={m.spawn_schedule_menu()}
				items={schedule.items}
				loading={sf.busy}
				disabled={sf.busy || !sf.valid}
				title={sf.disabledReason}
				primaryProps={{ 'aria-keyshortcuts': CHORD }}
				onclick={sf.submit}
			>
				{#if sf.form.auto_archive}
					<span class="archive-mark" data-journey="auto-archive-on">
						<Icon name="archive" />
						<span class="sr-only">{m.spawn_auto_archive()}</span>
					</span>
				{/if}
				{sf.spawnLabel}
			</SplitButton>
		{:else}
			<Button
				data-journey="submit"
				variant="primary"
				style={PRIMARY_END}
				loading={sf.busy}
				disabled={sf.busy || !sf.valid}
				title={sf.disabledReason}
				aria-keyshortcuts={CHORD}
				onclick={sf.submit}
			>
				{sf.spawnLabel}
			</Button>
		{/if}
	</div>
</div>

<style>
	.spawn-foot {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		flex: 1 1 100%;
		min-width: 0;
	}
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
		align-items: center;
		gap: var(--sp-2);
	}
	.foot-quiet {
		display: flex;
		align-items: center;
		gap: var(--sp-1);
		flex: none;
	}
	.archive-mark {
		display: inline-flex;
		margin-inline-end: var(--sp-1);
	}
</style>
