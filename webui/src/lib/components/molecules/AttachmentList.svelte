<script lang="ts">
	import { fileCapError } from '#lib/attachments.ts';
	import { previewFile } from '#lib/fileviewer.ts';
	import { uploadCaps } from '#lib/uploadCaps.svelte.ts';
	import type { PromptAttachments } from '#lib/promptAttachments.svelte.ts';
	import { AttachmentList } from '@dorsk/tsumikit';
	import Error from '#lib/components/atoms/Error.svelte';
	import ImageCompressionStatus from './ImageCompressionStatus.svelte';
	import { m } from '#lib/paraglide/messages.js';

	let { att }: { att: Pick<PromptAttachments, 'files' | 'images' | 'remove'> } = $props();

	const error = $derived(fileCapError(att.files, uploadCaps));
</script>

<ImageCompressionStatus pending={att.images.pending} />
{#if att.files.length}
	<AttachmentList
		files={att.files}
		tiles="auto"
		removeLabel={m.common_remove()}
		openLabel={m.common_open()}
		onremove={(i) => att.remove(att.files[i].name)}
		onopen={(i) => previewFile(att.files[i])}
	/>
{/if}
{#if error}<Error>{error}</Error>{/if}
