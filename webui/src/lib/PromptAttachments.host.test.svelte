<script lang="ts">
	import { untrack } from 'svelte';
	import { PromptAttachments, type PromptAttachmentsOpts } from './promptAttachments.svelte';

	let {
		opts,
		onready
	}: { opts: PromptAttachmentsOpts; onready: (a: PromptAttachments) => void } = $props();

	untrack(() => onready(new PromptAttachments(opts)));
</script>
