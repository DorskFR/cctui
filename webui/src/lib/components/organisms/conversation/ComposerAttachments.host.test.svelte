<script lang="ts">
	import { untrack } from 'svelte';
	import { ComposerAttachments, type ComposerAttachmentsOpts } from './composerAttachments.svelte';

	let {
		opts,
		onready
	}: { opts: ComposerAttachmentsOpts; onready: (a: ComposerAttachments) => void } = $props();

	untrack(() => onready(new ComposerAttachments(opts)));
</script>
