<script lang="ts">
	import { ComposerAttachments, type ComposerAttachmentsOpts } from './composerAttachments.svelte';

	let {
		opts,
		onready
	}: { opts: ComposerAttachmentsOpts; onready: (a: ComposerAttachments) => void } = $props();

	onready(new ComposerAttachments(opts));
</script>
