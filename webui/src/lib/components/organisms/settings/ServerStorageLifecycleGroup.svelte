<script lang="ts">
	import { Badge, Button, Callout, Input, Switch, Text, Timestamp } from '@dorsk/tsumikit';
	import type { StorageLifecycleInfo } from '@bindings/StorageLifecycleInfo';
	import type { StorageLifecyclePreview } from '@bindings/StorageLifecyclePreview';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import { errMessage } from '#lib/api.ts';
	import { useStorageLifecycleActions } from '#lib/queries/storageAdmin.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';
	import {
		MAX_DAYS,
		MAX_TRIM_KB,
		canSave,
		draftFrom,
		draftValid,
		enablesAnything,
		fmtBytes,
		isExact,
		rulesFrom,
		rulesKey,
		type LifecycleDraft
	} from './serverStorage.logic';

	let { info }: { info: StorageLifecycleInfo } = $props();

	const actions = useStorageLifecycleActions();

	const saved = $derived(draftFrom(info.rules));
	let trimOn = $derived(saved.trimOn);
	let trimDays = $derived(saved.trimDays);
	let trimKb = $derived(saved.trimKb);
	let dropOn = $derived(saved.dropOn);
	let dropDays = $derived(saved.dropDays);
	const draft: LifecycleDraft = $derived({ trimOn, trimDays, trimKb, dropOn, dropDays });

	let preview = $state<{ key: string; result: StorageLifecyclePreview } | null>(null);
	let busy = $state<'preview' | 'save' | null>(null);

	const rules = $derived(rulesFrom(draft));
	const key = $derived(rulesKey(rules));
	const shown = $derived(preview?.key === key ? preview.result : null);
	const saveable = $derived(canSave(draft, info.rules, preview?.key ?? null));
	const active = $derived(enablesAnything(info.rules));
	const run = $derived(info.last_run);
	const n = (v: number) => v.toLocaleString();

	async function runPreview() {
		const [asked, askedKey] = [rules, key];
		busy = 'preview';
		try {
			preview = { key: askedKey, result: await actions.preview(asked) };
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			busy = null;
		}
	}

	async function save() {
		busy = 'save';
		try {
			await actions.save(rules);
			toasts.ok(m.settings_server_storage_saved());
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			busy = null;
		}
	}
</script>

<SettingGroup title={m.settings_server_storage_lifecycle_group()}>
	<SettingRow
		label={m.settings_server_storage_lifecycle_label()}
		help={m.settings_server_storage_lifecycle_help()}
		selfLabelled
	>
		<Badge size="sm" tone={active ? 'warn' : 'ok'} data-testid="storage-policy">
			{active ? m.settings_server_storage_lifecycle_active() : m.settings_server_storage_lifecycle_keep_all()}
		</Badge>
	</SettingRow>

	<SettingRow label={m.settings_server_storage_trim_label()} help={m.settings_server_storage_trim_help()} server admin>
		<Switch bind:checked={trimOn} label={m.settings_server_storage_trim_label()} />
	</SettingRow>
	{#if trimOn}
		<SettingRow
			label={m.settings_server_storage_trim_days_label()}
			help={m.settings_server_storage_days_help({ max: String(MAX_DAYS) })}
		>
			<Input
				type="number"
				min="1"
				max={MAX_DAYS}
				step="1"
				bind:value={trimDays}
				aria-label={m.settings_server_storage_trim_days_label()}
			/>
		</SettingRow>
		<SettingRow label={m.settings_server_storage_trim_kb_label()} help={m.settings_server_storage_trim_kb_help()}>
			<Input
				type="number"
				min="1"
				max={MAX_TRIM_KB}
				step="1"
				bind:value={trimKb}
				aria-label={m.settings_server_storage_trim_kb_label()}
			/>
		</SettingRow>
	{/if}

	<SettingRow label={m.settings_server_storage_drop_label()} help={m.settings_server_storage_drop_help()} server admin>
		<Switch bind:checked={dropOn} label={m.settings_server_storage_drop_label()} />
	</SettingRow>
	{#if dropOn}
		<SettingRow
			label={m.settings_server_storage_drop_days_label()}
			help={m.settings_server_storage_days_help({ max: String(MAX_DAYS) })}
		>
			<Input
				type="number"
				min="1"
				max={MAX_DAYS}
				step="1"
				bind:value={dropDays}
				aria-label={m.settings_server_storage_drop_days_label()}
			/>
		</SettingRow>
	{/if}

	<SettingRow
		label={m.settings_server_storage_preview_label()}
		help={shown
			? isExact(shown.percent)
				? m.settings_server_storage_preview_exact()
				: m.settings_server_storage_preview_sampled({ percent: shown.percent.toPrecision(2) })
			: m.settings_server_storage_preview_help()}
		selfLabelled
	>
		<div class="actions">
			{#if shown}
				<Text size="sm" variant="code" data-testid="storage-preview">
					{m.settings_server_storage_preview_result({
						bytes: fmtBytes(shown.bytes_freed),
						trimmed: n(shown.rows_trimmed),
						dropped: n(shown.rows_dropped)
					})}
				</Text>
			{/if}
			<Button
				variant="ghost"
				disabled={!draftValid(draft) || !enablesAnything(rules) || busy !== null}
				loading={busy === 'preview'}
				onclick={runPreview}
			>
				{m.settings_server_storage_preview_button()}
			</Button>
			<Button disabled={!saveable || busy !== null} loading={busy === 'save'} onclick={save}>
				{m.settings_admin_instance_save()}
			</Button>
		</div>
	</SettingRow>

	<SettingRow
		label={m.settings_server_storage_last_run_label()}
		help={run
			? m.settings_server_storage_last_run_help({
					scanned: n(run.rows_scanned),
					trimmed: n(run.rows_trimmed),
					dropped: n(run.rows_dropped),
					skipped: n(run.rows_skipped)
				})
			: m.settings_server_storage_last_run_never()}
		selfLabelled
	>
		{#if run}
			<div class="actions">
				{#if !run.finished_at}
					<Badge size="sm" tone="info">{m.settings_server_storage_last_run_running()}</Badge>
				{/if}
				<Timestamp value={run.finished_at ?? run.started_at} mode="relative" size="xs" tone="faint" />
				<Text size="sm" variant="code">{fmtBytes(run.bytes_freed)}</Text>
			</div>
		{/if}
	</SettingRow>
	<SettingRow
		label={m.settings_server_storage_freed_label()}
		help={m.settings_server_storage_freed_help()}
		selfLabelled
	>
		<Text size="sm" variant="code" data-testid="storage-freed">{fmtBytes(info.bytes_freed_total)}</Text>
	</SettingRow>
</SettingGroup>
{#if run?.error}
	<Callout tone="danger" title={m.settings_server_storage_last_run_error({ error: run.error })} />
{/if}

<style>
	.actions {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: var(--sp-2);
		flex-wrap: wrap;
	}
</style>
