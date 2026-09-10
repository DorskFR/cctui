<script lang="ts">
	import { untrack } from 'svelte';
	import { Button, Field, Input, Modal, Textarea } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		title: initialTitle = '',
		note: initialNote = '',
		heading,
		saveLabel,
		onsave,
		onclose
	}: {
		title?: string;
		note?: string;
		heading: string;
		saveLabel: string;
		onsave: (title: string, note: string | null) => void;
		onclose: () => void;
	} = $props();

	// Mounted fresh per open by the caller's {#if}, so capturing the initial
	// props once is intended — untrack documents that.
	let title = $state(untrack(() => initialTitle));
	let note = $state(untrack(() => initialNote));
	let inputEl = $state<HTMLInputElement | null>(null);

	$effect(() => {
		inputEl?.focus();
		inputEl?.select();
	});

	function save() {
		if (title.trim() === '') return;
		onsave(title.trim(), note.trim() === '' ? null : note.trim());
	}
</script>

<Modal title={heading} {onclose} size="sm">
	{#snippet body()}
		<form
			class="stack body"
			onsubmit={(e) => {
				e.preventDefault();
				save();
			}}
		>
			<Field label={m.bookmarks_field_title()}>
				<Input bind:value={title} bind:el={inputEl} placeholder={m.bookmarks_title_placeholder()} />
			</Field>
			<Field label={m.bookmarks_field_note()} hint={m.bookmarks_note_hint()}>
				<Textarea bind:value={note} rows={3} />
			</Field>
		</form>
	{/snippet}
	{#snippet footer()}
		<Button block onclick={onclose}>{m.common_cancel()}</Button>
		<Button block variant="primary" onclick={save}>{saveLabel}</Button>
	{/snippet}
</Modal>

<style>
	.body {
		gap: var(--sp-3);
	}
</style>
