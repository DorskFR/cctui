<script lang="ts">
	import {
		Button,
		Callout,
		ConfirmModal,
		DataTable,
		EmptyState,
		Progress,
		SegmentedControl
	} from '@dorsk/tsumikit';
	import SettingRow from './SettingRow.svelte';
	import { useRescrub } from '$lib/queries/settings';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';
	import type { RescrubReport } from '@bindings/RescrubReport';
	import { rescrubScopeSince, rescrubCategoryRows, type RescrubScope } from './rescrub.logic';

	const rescrub = useRescrub();

	let scope = $state<RescrubScope>('all');
	let phase = $state<'idle' | 'scanning' | 'preview' | 'applying'>('idle');
	let confirming = $state(false);
	let report = $state<RescrubReport | null>(null);

	const rows = $derived(rescrubCategoryRows(report));

	const scopeOptions = $derived([
		{ value: 'all', label: m.settings_rescrub_scope_all() },
		{ value: '30d', label: m.settings_rescrub_scope_30d() },
		{ value: '7d', label: m.settings_rescrub_scope_7d() }
	]);

	async function scan() {
		phase = 'scanning';
		try {
			report = await rescrub({ dry_run: true, session_ids: null, since: rescrubScopeSince(scope) });
			phase = 'preview';
		} catch (e) {
			phase = 'idle';
			toasts.error(e instanceof Error ? e.message : String(e));
		}
	}

	async function apply() {
		confirming = false;
		phase = 'applying';
		try {
			const done = await rescrub({
				dry_run: false,
				session_ids: null,
				since: rescrubScopeSince(scope)
			});
			toasts.ok(
				m.settings_rescrub_toast_done({
					values: done.substitutions,
					messages: done.rows_changed
				})
			);
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		} finally {
			report = null;
			phase = 'idle';
		}
	}

	function cancel() {
		report = null;
		phase = 'idle';
	}
</script>

<SettingRow
	label={m.settings_rescrub_label()}
	help={m.settings_rescrub_help()}
	server
	wide
	selfLabelled
>
	<div class="panel">
		<div class="controls">
			<SegmentedControl
				options={scopeOptions}
				value={scope}
				onchange={(v) => (scope = v as RescrubScope)}
				label={m.settings_rescrub_scope_label()}
			/>
			<Button
				tone="accent"
				loading={phase === 'scanning'}
				disabled={phase !== 'idle'}
				onclick={scan}
			>
				{m.settings_rescrub_scan()}
			</Button>
		</div>

		{#if phase === 'scanning' || phase === 'applying'}
			<Progress block indeterminate label={m.settings_rescrub_scanning()} />
		{/if}

		{#if phase === 'preview' && report}
			{#if report.substitutions === 0}
				<EmptyState size="compact" title={m.settings_rescrub_empty()} />
				<Button onclick={cancel}>{m.common_cancel()}</Button>
			{:else}
				<Callout tone="info" title={m.settings_rescrub_preview_title()}>
					{m.settings_rescrub_preview_body({
						scanned: report.rows_scanned,
						changed: report.rows_changed,
						values: report.substitutions
					})}
				</Callout>
				<DataTable
					size="sm"
					columns={[
						{ key: 'category', label: m.settings_rescrub_col_category() },
						{ key: 'count', label: m.settings_rescrub_col_count(), align: 'right' }
					]}
					{rows}
					rowKey={(r) => r.category}
				/>
				<div class="controls">
					<Button tone="warn" onclick={() => (confirming = true)}>
						{m.settings_rescrub_apply({ count: report.rows_changed })}
					</Button>
					<Button tone="neutral" onclick={cancel}>{m.common_cancel()}</Button>
				</div>
			{/if}
		{/if}
	</div>
</SettingRow>

<ConfirmModal
	bind:open={confirming}
	tone="warn"
	title={m.settings_rescrub_confirm_title({ count: report?.rows_changed ?? 0 })}
	message={m.settings_rescrub_confirm_body()}
	confirmLabel={m.settings_rescrub_confirm_ok()}
	onconfirm={apply}
	oncancel={() => (confirming = false)}
/>

<style>
	.panel {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		width: 100%;
	}
	.controls {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
	}
</style>
