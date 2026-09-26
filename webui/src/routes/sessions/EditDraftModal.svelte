<script lang="ts">
	import { Button, Cluster, Modal, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import type { SessionsPage } from './sessionsPage.svelte';

	// Asks what to do with the spawn form's current content before it is
	// replaced by the draft being edited.
	let { sp }: { sp: SessionsPage } = $props();
</script>

<Modal title={m.sessions_edit_draft_title()} onclose={() => (sp.pendingDraftEdit = null)} footerFill>
	{#snippet body()}
		<Text>{m.sessions_edit_draft_body()}</Text>
	{/snippet}
	{#snippet footer()}
		<Cluster>
			<Button grow onclick={() => (sp.pendingDraftEdit = null)}>{m.common_cancel()}</Button>
			<Button grow onclick={() => void sp.confirmDraftEdit(true)}>{m.sessions_edit_draft_save_first()}</Button>
			<Button grow variant="primary" onclick={() => void sp.confirmDraftEdit(false)}
				>{m.sessions_edit_draft_replace()}</Button
			>
		</Cluster>
	{/snippet}
</Modal>
