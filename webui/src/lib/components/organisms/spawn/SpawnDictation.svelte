<script lang="ts">
	import VoiceControls from '../conversation/VoiceControls.svelte';
	import { useVoiceConfig } from '$lib/queries';
	import { settings } from '$lib/settings.svelte';
	import { voiceCaps } from '$lib/voice/capabilities';
	import { dictateIntoPrompt, type DictationTarget } from './spawnDictation';

	let { sf }: { sf: DictationTarget & { busy: boolean } } = $props();

	const voiceConfig = useVoiceConfig();
	const caps = $derived(voiceCaps(voiceConfig.data));
</script>

{#if caps.stt}
	<div class="spawn-dictation" data-journey="spawn-dictation">
		<VoiceControls
			disabled={sf.busy}
			ondictated={(text) => void dictateIntoPrompt(sf, text, settings.voice.autoSend)}
		/>
	</div>
{/if}

<style>
	.spawn-dictation {
		display: flex;
		flex-basis: 100%;
		justify-content: flex-end;
	}
</style>
