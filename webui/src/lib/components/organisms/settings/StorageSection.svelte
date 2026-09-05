<script lang="ts">
	// What this browser holds locally, shown as a group of the Instance page.
	// Today that is the IndexedDB attachment cache behind unsent drafts; the
	// readout is recomputed after a clear so the figure never lies.
	import { Button, Text } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import { attachmentStore } from '$lib/attachmentStore';
	import { fmtSize } from '$lib/attachments';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';

	let bytes = $state(0);
	let busy = $state(false);

	async function refresh() {
		bytes = await attachmentStore.totalBytes();
	}
	$effect(() => {
		void refresh();
	});

	async function clear() {
		busy = true;
		try {
			await attachmentStore.clearAll();
			await refresh();
			toasts.ok(m.settings_storage_cleared());
		} finally {
			busy = false;
		}
	}
</script>

<SettingGroup title={m.settings_storage_title()}>
	<SettingRow
		label={m.settings_storage_attachments_label()}
		help={m.settings_storage_attachments_help()}
	>
		<div class="ctl">
			<Text size="sm" tone="faint">
				{bytes > 0 ? m.settings_storage_used({ size: fmtSize(bytes) }) : m.settings_storage_empty()}
			</Text>
			<Button size="sm" variant="ghost" disabled={busy || bytes === 0} onclick={clear}>
				{m.settings_storage_clear()}
			</Button>
		</div>
	</SettingRow>
</SettingGroup>

<style>
	.ctl {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-3);
	}
</style>
