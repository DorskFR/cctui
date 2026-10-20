<script lang="ts">
	// Composer voice entry points: hold the mic to dictate into the draft,
	// flip the toggle to talk with the session hands-free.
	import { IconButton, Toggle } from '@dorsk/tsumikit';
	import { errMessage } from '$lib/api';
	import { toasts } from '$lib/toast.svelte';
	import { Dictation } from '$lib/voice/dictation';
	import { m } from '$lib/paraglide/messages';

	let {
		live,
		ontogglelive,
		ondictated,
		disabled = false
	}: {
		live: boolean;
		ontogglelive: () => void;
		/** A finished dictation; empty clips never reach here. */
		ondictated: (text: string) => void;
		disabled?: boolean;
	} = $props();

	const dictation = new Dictation();
	let recording = $state(false);
	let busy = $state(false);
	$effect(() => () => dictation.cancel());

	async function begin() {
		if (disabled || busy || recording || live) return;
		try {
			await dictation.start();
			recording = dictation.recording;
		} catch (e) {
			toasts.error(errMessage(e));
		}
	}

	async function finish() {
		if (!recording) {
			dictation.cancel();
			return;
		}
		recording = false;
		busy = true;
		try {
			const text = await dictation.stop();
			if (text) ondictated(text);
			else toasts.info(m.voice_dictation_empty());
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			busy = false;
		}
	}

	function onkey(e: KeyboardEvent) {
		if (e.key !== 'Enter' && e.key !== ' ') return;
		e.preventDefault();
		if (recording) void finish();
		else void begin();
	}
</script>

<span class="voice-controls" data-journey="voice-controls">
	<IconButton
		emoji="🎙"
		box="sm"
		variant="ghost"
		pressed={recording}
		loading={busy}
		disabled={disabled || live}
		label={m.voice_dictate_label()}
		title={m.voice_dictate_title()}
		style={recording ? '--btn-on: var(--danger)' : undefined}
		onpointerdown={(e: PointerEvent) => {
			e.preventDefault();
			void begin();
		}}
		onpointerup={() => void finish()}
		onpointercancel={() => void finish()}
		onpointerleave={() => {
			if (recording) void finish();
		}}
		onkeydown={onkey}
	/>
	<Toggle
		size="sm"
		pressed={live}
		{disabled}
		aria-label={m.voice_live_toggle_title()}
		title={m.voice_live_toggle_title()}
		onclick={ontogglelive}>{m.voice_live_toggle()}</Toggle
	>
</span>

<style>
	.voice-controls {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
	}
</style>
