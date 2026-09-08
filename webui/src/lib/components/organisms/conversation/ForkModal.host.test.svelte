<script lang="ts">
	import { untrack } from 'svelte';
	import ForkModal from './ForkModal.svelte';
	import type { ModelOption } from '$lib/harnessModels';

	let {
		models,
		model: initialModel,
		onmodel
	}: { models: ModelOption[]; model: string; onmodel: (v: string) => void } = $props();

	let model = $state(untrack(() => initialModel));
	let effort = $state('');
	$effect(() => onmodel(model));
</script>

<ForkModal
	archived={false}
	isCodexSession={false}
	parentTokens={0}
	{models}
	efforts={['', 'high']}
	forking={false}
	bind:model
	bind:effort
	oncancel={() => {}}
	onsubmit={() => {}}
/>
