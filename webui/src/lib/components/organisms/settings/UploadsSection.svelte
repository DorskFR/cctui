<script lang="ts">
	// Settings › Instance › Limits, uploads: the three per-upload soft caps, plus read-only copy
	// for the two limits this page cannot move — the router's boot-time body
	// ceiling (env + restart) and the daemon's own compiled-in limits.
	import { Badge, Button, Input, Text } from '@dorsk/tsumikit';
	import { useQueryClient } from '@tanstack/svelte-query';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import { endpoints, qk, useVersion } from '#lib/queries/index.ts';
	import { fmtSize, DEFAULT_UPLOAD_CAPS } from '#lib/attachments.ts';
	import { autoSave, serverSave } from './serverSave.svelte.ts';
	import type { UploadCapsInfo } from '@bindings/UploadCapsInfo';
	import { m } from '#lib/paraglide/messages.js';
	import { sourceLabel } from './serverSettings.logic';

	let { isAdmin = false }: { isAdmin?: boolean } = $props();

	const MB = 1024 * 1024;
	const toMb = (bytes: number) => Math.round((bytes / MB) * 100) / 100;

	const version = useVersion();
	const qc = useQueryClient();
	const served = $derived(version.data?.upload_caps ?? DEFAULT_UPLOAD_CAPS);

	let info = $state<UploadCapsInfo | null>(null);
	let files = $state(0);
	let fileMb = $state(0);
	let totalMb = $state(0);
	function fill(caps: { max_files: number; max_file_bytes: number; max_total_bytes: number }) {
		files = caps.max_files;
		fileMb = toMb(caps.max_file_bytes);
		totalMb = toMb(caps.max_total_bytes);
	}
	function apply(next: UploadCapsInfo) {
		info = next;
		fill(next.effective);
	}
	$effect(() => {
		if (!isAdmin) return;
		endpoints
			.uploadCaps()
			.then(apply)
			.catch(() => {});
	});
	$effect(() => {
		if (!info) fill(served);
	});

	const effective = $derived(info?.effective ?? served);

	const draft = $derived({
		max_files: Math.trunc(files),
		max_file_bytes: Math.round(fileMb * MB),
		max_total_bytes: Math.round(totalMb * MB)
	});
	const dirty = $derived(
		draft.max_files !== effective.max_files ||
			draft.max_file_bytes !== effective.max_file_bytes ||
			draft.max_total_bytes !== effective.max_total_bytes
	);
	const valid = $derived(
		draft.max_files > 0 && draft.max_file_bytes > 0 && draft.max_total_bytes > 0
	);

	type Caps = typeof draft;
	async function save(caps: Caps | null) {
		const sent = JSON.stringify(draft);
		const res = await serverSave.run(async () => {
			const next = await endpoints.setUploadCaps(caps);
			await qc.invalidateQueries({ queryKey: qk.version });
			return next;
		});
		if (!res) return;
		if (JSON.stringify(draft) === sent) apply(res);
		else info = res;
	}
	autoSave(() => (isAdmin && info && dirty && valid ? draft : undefined), save);
</script>

	<SettingGroup title={`${m.settings_nav_uploads()} · ${m.settings_uploads_group_now()}`}>
	<SettingRow label={m.settings_uploads_max_files_label()} help={m.settings_uploads_max_files_help()}>
		{#if isAdmin}
			<Input type="number" min="1" step="1" bind:value={files} aria-label={m.settings_uploads_max_files_label()} />
		{:else}
			<Text size="sm" numeric>{effective.max_files}</Text>
		{/if}
	</SettingRow>
	<SettingRow
		label={m.settings_uploads_max_file_label()}
		help={m.settings_uploads_max_file_help()}
	>
		{#if isAdmin}
			<Input type="number" min="1" step="1" bind:value={fileMb} aria-label={m.settings_uploads_max_file_label()} />
		{:else}
			<Text size="sm" numeric>{fmtSize(effective.max_file_bytes)}</Text>
		{/if}
	</SettingRow>
	<SettingRow
		label={m.settings_uploads_max_total_label()}
		help={m.settings_uploads_max_total_help()}
	>
		{#if isAdmin}
			<Input type="number" min="1" step="1" bind:value={totalMb} aria-label={m.settings_uploads_max_total_label()} />
		{:else}
			<Text size="sm" numeric>{fmtSize(effective.max_total_bytes)}</Text>
		{/if}
	</SettingRow>
	{#if isAdmin}
		<SettingRow label={m.settings_uploads_apply_label()} help={m.settings_uploads_apply_help()} server admin selfLabelled>
			<div class="actions">
				{#if info}
					<Badge size="sm">{sourceLabel(info.source)}</Badge>
				{/if}
				{#if info?.source === 'settings'}
					<Button variant="ghost" disabled={serverSave.status === 'pending'} onclick={() => save(null)}>
						{m.settings_reset()}
					</Button>
				{/if}
			</div>
		</SettingRow>
	{/if}
</SettingGroup>

<SettingGroup title={`${m.settings_nav_uploads()} · ${m.settings_uploads_group_restart()}`}>
	<SettingRow
		label={m.settings_uploads_ceiling_label()}
		help={info
			? m.settings_uploads_ceiling_help({ env: info.body_limit_env })
			: m.settings_uploads_ceiling_help_generic()}
		selfLabelled
	>
		<Text size="sm" numeric tone="faint">
			{info ? fmtSize(info.body_limit_bytes) : '—'}
		</Text>
	</SettingRow>
</SettingGroup>

<SettingGroup title={`${m.settings_nav_uploads()} · ${m.settings_uploads_group_elsewhere()}`}>
	<SettingRow label={m.settings_uploads_daemon_label()} help={m.settings_uploads_daemon_help()} selfLabelled>
		<Text size="sm" tone="faint">{m.settings_uploads_fixed()}</Text>
	</SettingRow>
	<SettingRow label={m.settings_uploads_images_label()} help={m.settings_uploads_images_help()} selfLabelled>
		<Text size="sm" tone="faint">{m.settings_uploads_fixed()}</Text>
	</SettingRow>
</SettingGroup>

<style>
	.actions {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: var(--sp-2);
		flex-wrap: wrap;
	}
</style>
