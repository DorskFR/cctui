<script lang="ts">
	import { untrack } from 'svelte';
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { Button, Field, Input, Modal, Text } from '@dorsk/tsumikit';
	import { useSessionActions } from '$lib/queries';
	import { readPluginSlot } from '$lib/plugins/sessionSlots';
	import { YOUTRACK_PLUGIN_ID, resolveIssueSlot } from '$lib/plugins/issueLink';
	import { m } from '$lib/paraglide/messages';

	let {
		session,
		detected = null,
		onclose
	}: { session: SessionListItem; detected?: string | null; onclose: () => void } = $props();

	const actions = useSessionActions();
	const current = $derived(readPluginSlot(session.metadata, YOUTRACK_PLUGIN_ID));
	const currentIssue = $derived(typeof current?.issue === 'string' ? current.issue : null);

	let entry = $state(untrack(() => currentIssue ?? detected ?? ''));
	let saving = $state(false);
	let error = $state<string | null>(null);

	async function save() {
		const raw = entry.trim();
		saving = true;
		error = null;
		try {
			const data = raw ? await resolveIssueSlot(raw) : null;
			if (raw && !data) {
				error = m.plugin_issue_invalid();
				return;
			}
			await actions.setPluginSlot(session.id, YOUTRACK_PLUGIN_ID, data);
			onclose();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			saving = false;
		}
	}
</script>

<Modal title={m.plugin_issue_modal_title()} busy={saving} {onclose}>
	{#snippet body()}
		<div class="il-body" data-journey="issue-link-modal">
			<Text as="p" tone="muted" size="sm">{m.plugin_issue_modal_help()}</Text>
			<Field label={m.plugin_issue_modal_field()}>
				<Input
					bind:value={entry}
					placeholder={m.plugin_chip_issue_placeholder()}
					aria-label={m.plugin_chip_issue_aria()}
					aria-invalid={!!error}
					disabled={saving}
					onsubmit={save}
				/>
			</Field>
			{#if currentIssue}
				<Text size="xs" tone="faint">{m.plugin_issue_modal_current({ issue: currentIssue })}</Text>
			{/if}
			{#if error}
				<div role="alert"><Text size="sm" tone="danger">{error}</Text></div>
			{/if}
		</div>
	{/snippet}
	{#snippet footer()}
		<Button size="sm" variant="ghost" onclick={onclose} disabled={saving}>{m.common_cancel()}</Button>
		<Button size="sm" variant="default" onclick={save} loading={saving} disabled={saving}>
			{m.common_save()}
		</Button>
	{/snippet}
</Modal>

<style>
	.il-body {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
</style>
