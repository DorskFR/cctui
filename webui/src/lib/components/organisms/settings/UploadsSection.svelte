<script lang="ts">
	// Instance › Limits › Uploads: the three per-upload soft caps, saved when a
	// field is committed, plus read-only copy for the limits this page cannot
	// move — the router's boot-time body ceiling (env + restart) and the
	// daemon's own compiled-in limits.
	import { Badge, Button, Input, Text } from '@dorsk/tsumikit';
	import { useQueryClient } from '@tanstack/svelte-query';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import { endpoints, qk, useVersion } from '$lib/queries';
	import { fmtSize, DEFAULT_UPLOAD_CAPS } from '$lib/attachments';
	import { toasts } from '$lib/toast.svelte';
	import type { UploadCapsInfo } from '@bindings/UploadCapsInfo';
	import { m } from '$lib/paraglide/messages';
	import { sourceLabel } from './serverSettings.logic';

	const MB = 1024 * 1024;
	const toMb = (bytes: number) => Math.round((bytes / MB) * 100) / 100;

	const version = useVersion();
	const qc = useQueryClient();
	const served = $derived(version.data?.upload_caps ?? DEFAULT_UPLOAD_CAPS);

	let info = $state<UploadCapsInfo | null>(null);
	let saving = $state(false);
	$effect(() => {
		endpoints
			.uploadCaps()
			.then((next) => (info = next))
			.catch(() => {});
	});

	const effective = $derived(info?.effective ?? served);

	let files = $state(0);
	let fileMb = $state(0);
	let totalMb = $state(0);
	$effect(() => {
		const e = effective;
		files = e.max_files;
		fileMb = toMb(e.max_file_bytes);
		totalMb = toMb(e.max_total_bytes);
	});

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

	function commit() {
		if (dirty && valid && !saving) void save();
	}

	async function save(reset = false) {
		saving = true;
		try {
			info = await endpoints.setUploadCaps(reset ? null : draft);
			await qc.invalidateQueries({ queryKey: qk.version });
			toasts.ok(m.settings_uploads_saved());
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		} finally {
			saving = false;
		}
	}
</script>

<div id="uploads">
	<SettingGroup title={m.settings_uploads_group_now()}>
		<SettingRow label={m.settings_uploads_max_files_label()} help={m.settings_uploads_max_files_help()}>
			<Input type="number" min="1" step="1" bind:value={files} onchange={commit} aria-label={m.settings_uploads_max_files_label()} />
		</SettingRow>
		<SettingRow label={m.settings_uploads_max_file_label()} help={m.settings_uploads_max_file_help()}>
			<Input type="number" min="1" step="1" bind:value={fileMb} onchange={commit} aria-label={m.settings_uploads_max_file_label()} />
		</SettingRow>
		<SettingRow label={m.settings_uploads_max_total_label()} help={m.settings_uploads_max_total_help()}>
			<Input type="number" min="1" step="1" bind:value={totalMb} onchange={commit} aria-label={m.settings_uploads_max_total_label()} />
		</SettingRow>
		{#if info?.source === 'settings'}
			<SettingRow label={m.set_uploads_reset_label()} help={m.settings_uploads_apply_help()} selfLabelled>
				<div class="actions">
					<Badge size="sm">{sourceLabel(info.source)}</Badge>
					<Button variant="ghost" disabled={saving} onclick={() => save(true)}>
						{m.settings_reset()}
					</Button>
				</div>
			</SettingRow>
		{/if}
	</SettingGroup>

	<SettingGroup title={m.settings_uploads_group_restart()}>
		<SettingRow
			label={m.settings_uploads_ceiling_label()}
			help={info
				? m.settings_uploads_ceiling_help({ env: info.body_limit_env })
				: m.settings_uploads_ceiling_help_generic()}
			selfLabelled
		>
			<Text size="sm" tone="faint" class="num">
				{info ? fmtSize(info.body_limit_bytes) : '—'}
			</Text>
		</SettingRow>
	</SettingGroup>

	<SettingGroup title={m.settings_uploads_group_elsewhere()}>
		<SettingRow label={m.settings_uploads_daemon_label()} help={m.settings_uploads_daemon_help()} selfLabelled>
			<Text size="sm" tone="faint">{m.settings_uploads_fixed()}</Text>
		</SettingRow>
		<SettingRow label={m.settings_uploads_images_label()} help={m.settings_uploads_images_help()} selfLabelled>
			<Text size="sm" tone="faint">{m.settings_uploads_fixed()}</Text>
		</SettingRow>
	</SettingGroup>
</div>

<style>
	.actions {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: var(--sp-2);
		flex-wrap: wrap;
	}
</style>
