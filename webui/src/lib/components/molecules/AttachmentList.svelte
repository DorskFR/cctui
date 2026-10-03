<script lang="ts">
	// Pending attachments for the spawn modal and the composer: the kit list
	// (numbered like the `[#N]` prompt markers, tiles when narrow, click to
	// preview) plus the upload cap error.
	import { fileCapError } from '$lib/attachments';
	import { previewFile } from '$lib/fileviewer';
	import { uploadCaps } from '$lib/uploadCaps.svelte';
	import { AttachmentList } from '@dorsk/tsumikit';
	import Error from '$lib/components/atoms/Error.svelte';
	import { m } from '$lib/paraglide/messages';

	let { files, onremove }: { files: File[]; onremove: (name: string) => void } = $props();

	const error = $derived(fileCapError(files, uploadCaps));
</script>

{#if files.length}
	<AttachmentList
		{files}
		tiles="auto"
		numbered
		removeLabel={m.common_remove()}
		openLabel={m.common_open()}
		onremove={(i) => onremove(files[i].name)}
		onopen={(i) => previewFile(files[i])}
	/>
{/if}
{#if error}<Error>{error}</Error>{/if}
