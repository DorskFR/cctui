<script lang="ts">
	import { onMount } from 'svelte';
	import {
		Button,
		Callout,
		ConfirmModal,
		EmptyState,
		Progress,
		SegmentedControl,
		Text
	} from '@dorsk/tsumikit';
	import SettingRow from './SettingRow.svelte';
	import { useRescrub } from '#lib/queries/settings.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { settings } from '#lib/settings.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';
	import type { PrivacyScanJob } from '@bindings/PrivacyScanJob';
	import type { ScanCategory } from '@bindings/ScanCategory';
	import {
		rescrubCategoryRows,
		rescrubHasFindings,
		rescrubIdentifierWarnings,
		rescrubIsRunning,
		rescrubLocal,
		rescrubMachineLine,
		rescrubProgress,
		rescrubScopeSince,
		rescrubSettleNotice,
		type RescrubMachineLine,
		type RescrubScope
	} from './rescrub.logic';

	const rescrub = useRescrub();

	let scope = $state<RescrubScope>('all');
	let job = $state<PrivacyScanJob | null>(null);
	let confirming = $state(false);
	let starting = $state(false);
	let watched: string | null = null;

	const running = $derived(rescrubIsRunning(job));
	const progress = $derived(rescrubProgress(job));
	const categories = $derived(rescrubCategoryRows(job));
	const warnings = $derived(rescrubIdentifierWarnings(job));
	const preview = $derived(
		job?.status === 'completed' && job.dry_run && rescrubHasFindings(job) ? job : null
	);
	const local = $derived(rescrubLocal(job));

	function machineText(line: RescrubMachineLine): string {
		if (line.kind === 'pending') return m.settings_rescrub_local_pending();
		if (line.kind === 'scanned') {
			const counts = { files: line.files, values: line.values, changed: line.changed };
			return line.dryRun
				? m.settings_rescrub_local_scanned(counts)
				: m.settings_rescrub_local_scrubbed(counts);
		}
		switch (line.reason) {
			case 'offline':
				return m.settings_rescrub_local_offline();
			case 'outdated':
				return m.settings_rescrub_local_outdated();
			case 'timeout':
				return m.settings_rescrub_local_timeout();
			case 'cancelled':
				return m.settings_rescrub_local_cancelled();
			case 'failed':
				return m.settings_rescrub_local_failed({ error: line.detail ?? '' });
		}
	}

	const scopeOptions = $derived([
		{ value: 'all', label: m.settings_rescrub_scope_all() },
		{ value: '30d', label: m.settings_rescrub_scope_30d() },
		{ value: '7d', label: m.settings_rescrub_scope_7d() }
	]);

	function settle(done: PrivacyScanJob) {
		const notice = rescrubSettleNotice(done, watched);
		watched = null;
		if (notice) rescrub.settled(done);
		if (notice === 'failed') {
			toasts.error(m.settings_rescrub_failed({ error: done.error ?? '' }));
		} else if (notice === 'done') {
			toasts.ok(
				m.settings_rescrub_toast_done({ values: done.substitutions, messages: done.rows_changed })
			);
		}
	}

	async function refresh() {
		try {
			const next = await rescrub.poll();
			job = next;
			if (next?.status === 'running') watched = next.id;
			else if (next) settle(next);
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		}
	}

	// A scan outlives the page: reopening Settings re-attaches to whatever the
	// server still has running, on whichever replica answers.
	onMount(() => {
		void refresh();
	});

	$effect(() => {
		if (!running) return;
		const timer = setInterval(refresh, 1000);
		return () => clearInterval(timer);
	});

	async function startScan(dry_run: boolean) {
		starting = true;
		try {
			job = await rescrub.start({ dry_run, session_ids: null, since: rescrubScopeSince(scope) });
			watched = job.id;
			if (job.status !== 'running') settle(job);
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		} finally {
			starting = false;
		}
	}

	async function cancelScan() {
		try {
			const cancelled = await rescrub.cancel();
			if (cancelled) job = cancelled;
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		}
	}

	function apply() {
		confirming = false;
		void startScan(false);
	}

	function dismiss() {
		job = null;
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
				loading={starting}
				disabled={running || starting}
				onclick={() => startScan(true)}
			>
				{m.settings_rescrub_scan()}
			</Button>
		</div>

		{#if settings.secretScrubOutbound}
			<Text size="sm" tone="muted">{m.settings_rescrub_outbound_note()}</Text>
		{/if}

		{#if running && job}
			<Progress
				block
				indeterminate={progress === null}
				value={progress ? progress.value : 0}
				max={progress ? progress.max : 1}
				label={job.dry_run ? m.settings_rescrub_scanning() : m.settings_rescrub_applying()}
			/>
			<div class="controls">
				<Text size="sm" tone="muted" as="span">
					{#if progress}
						{m.settings_rescrub_progress({
							scanned: progress.value,
							total: progress.max,
							changed: job.rows_changed
						})}
					{:else}
						{m.settings_rescrub_scanning()}
					{/if}
				</Text>
				<Button tone="neutral" onclick={cancelScan}>{m.settings_rescrub_cancel()}</Button>
			</div>
		{/if}

		{#if job?.status === 'cancelled'}
			<Callout tone="warn" title={m.settings_rescrub_cancelled_title()}>
				{m.settings_rescrub_cancelled_body({
					scanned: job.rows_scanned,
					changed: job.rows_changed
				})}
			</Callout>
			<Button onclick={dismiss}>{m.common_close()}</Button>
		{/if}

		{#if job?.status === 'completed' && job.dry_run && !rescrubHasFindings(job)}
			<EmptyState size="compact" title={m.settings_rescrub_empty()} />
			<Button onclick={dismiss}>{m.common_close()}</Button>
		{/if}

		{#if preview}
			<Callout tone="info" title={m.settings_rescrub_preview_title()}>
				{m.settings_rescrub_preview_body({
					scanned: preview.rows_scanned,
					changed: preview.rows_changed,
					values: preview.substitutions
				})}
			</Callout>
			{#if warnings.length > 0}
				<Callout tone="warn" title={m.settings_rescrub_identifier_title()}>
					{m.settings_rescrub_identifier_body({ categories: warnings.join(', ') })}
				</Callout>
			{/if}
			{@render categoryList(categories)}
			<div class="controls">
				<Button tone="warn" onclick={() => (confirming = true)}>
					{m.settings_rescrub_apply({ count: preview.rows_changed })}
				</Button>
				<Button tone="neutral" onclick={dismiss}>{m.common_cancel()}</Button>
			</div>
		{/if}

		{#if local}
			<section class="local">
				<Text size="sm" weight="semibold">{m.settings_rescrub_local_title()}</Text>
				<Text size="xs" tone="muted">{m.settings_rescrub_local_help()}</Text>
				{#if local.machines.length === 0}
					<Text size="sm" tone="muted">{m.settings_rescrub_local_none()}</Text>
				{/if}
				<ul class="machines">
					{#each local.machines as machine (machine.machine_id)}
						{@const line = rescrubMachineLine(machine)}
						<li class="machine">
							<Text size="sm" weight="semibold" as="span">{machine.machine_name}</Text>
							<Text size="sm" tone={line.kind === 'not_scanned' ? 'warn' : 'muted'} as="span">
								{machineText(line)}
							</Text>
							{#if line.kind === 'scanned' && line.live > 0}
								<Text size="xs" tone="faint" as="span">
									{m.settings_rescrub_local_live({ count: line.live })}
								</Text>
							{/if}
						</li>
					{/each}
				</ul>
				{#if !running && local.categories.length > 0}
					{@render categoryList(local.categories)}
				{/if}
			</section>
		{/if}
	</div>
</SettingRow>

{#snippet categoryList(rows: ScanCategory[])}
	<ul class="categories">
		{#each rows as cat (cat.category)}
			<li class="category">
				<div class="head">
					<Text size="sm" weight="semibold" as="span">{cat.category}</Text>
					<Text size="sm" tone="muted" as="span">
						{m.settings_rescrub_matches({ count: cat.count })}
					</Text>
				</div>
				<ul class="samples">
					{#each cat.samples as sample, i (i)}
						<li>
							<code class="match">{sample.text}</code>
							<code class="context">{sample.context}</code>
						</li>
					{/each}
				</ul>
			</li>
		{/each}
	</ul>
{/snippet}

<ConfirmModal
	bind:open={confirming}
	tone="warn"
	title={m.settings_rescrub_confirm_title({ count: preview?.rows_changed ?? 0 })}
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
	.local {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.machine {
		display: flex;
		flex-wrap: wrap;
		align-items: baseline;
		gap: var(--sp-2);
	}
	.categories,
	.machines,
	.samples {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	.category {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		padding: var(--sp-2);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
	}
	.head {
		display: flex;
		justify-content: space-between;
		gap: var(--sp-2);
	}
	.samples li {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-2);
		align-items: baseline;
	}
	.match {
		font-size: var(--fs-xs);
		color: var(--warn);
	}
	.context {
		font-size: var(--fs-xs);
		color: var(--text-muted);
		overflow-wrap: anywhere;
	}
</style>
