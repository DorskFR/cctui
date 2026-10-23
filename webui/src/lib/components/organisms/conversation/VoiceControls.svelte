<script lang="ts">
	// Composer voice entry points: the mic dictates into the draft (tap to
	// start, tap to send, or hold to talk), the toggle talks with the session
	// hands-free. While recording, a large stop target sits above the composer.
	import { Button, IconButton, Toggle } from '@dorsk/tsumikit';
	import { errMessage } from '$lib/api';
	import { toasts } from '$lib/toast.svelte';
	import { Dictation } from '$lib/voice/dictation';
	import { TapHold, type TapHoldAction } from '$lib/voice/tapHold';
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
	const gesture = new TapHold();
	let recording = $state(false);
	let busy = $state(false);
	let startedAt = $state(0);
	let elapsed = $state(0);
	$effect(() => () => dictation.cancel());
	$effect(() => {
		if (!recording) return;
		const tick = setInterval(() => (elapsed = Date.now() - startedAt), 250);
		return () => clearInterval(tick);
	});

	const timer = $derived.by(() => {
		const s = Math.floor(elapsed / 1000);
		return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
	});

	function act(action: TapHoldAction) {
		switch (action) {
			case 'start':
				void begin();
				break;
			case 'stop':
				void finish();
				break;
			case 'cancel':
				discard();
				break;
		}
	}

	function armed(): boolean {
		return !(disabled || busy || live);
	}

	async function begin() {
		if (!armed()) {
			gesture.reset();
			return;
		}
		startedAt = Date.now();
		elapsed = 0;
		recording = true;
		try {
			await dictation.start();
			if (!gesture.recording) return;
			recording = dictation.recording;
		} catch (e) {
			gesture.reset();
			recording = false;
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

	function discard() {
		gesture.reset();
		recording = false;
		dictation.cancel();
	}

	function onkey(e: KeyboardEvent) {
		if (e.key !== 'Enter' && e.key !== ' ') return;
		e.preventDefault();
		act(gesture.toggle(Date.now()));
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
		label={m.voice_dictate_hint()}
		title={m.voice_dictate_hint()}
		style={recording ? '--btn-on: var(--danger)' : undefined}
		onpointerdown={(e: PointerEvent) => {
			e.preventDefault();
			if (armed()) act(gesture.down(Date.now()));
		}}
		onpointerup={() => act(gesture.up(Date.now()))}
		onpointercancel={() => act(gesture.up(Date.now()))}
		onpointerleave={() => act(gesture.up(Date.now()))}
		onkeydown={onkey}
	/>
	<Toggle
		size="sm"
		pressed={live}
		{disabled}
		aria-label={m.voice_live_hint()}
		title={m.voice_live_hint()}
		onclick={ontogglelive}>{m.voice_live_toggle()}</Toggle
	>
</span>

{#if recording}
	<div class="recording" data-journey="voice-recording" role="status" aria-live="polite">
		<div class="stop">
			<Button
				variant="danger"
				size="lg"
				pill
				grow
				aria-label={m.voice_dictate_stop()}
				title={m.voice_dictate_stop()}
				onclick={() => act(gesture.toggle(Date.now()))}
			>
				<span class="dot" aria-hidden="true"></span>
				<span class="stop-label">{m.voice_dictate_stop()}</span>
				<span class="timer" aria-label={m.voice_dictate_recording()}>{timer}</span>
			</Button>
		</div>
		<IconButton
			icon="x"
			box="lg"
			variant="ghost"
			label={m.voice_dictate_discard()}
			title={m.voice_dictate_discard()}
			onclick={discard}
		/>
	</div>
{/if}

<style>
	.voice-controls {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
	}
	.recording {
		position: fixed;
		inset-inline: var(--sp-3);
		bottom: calc(7rem + env(safe-area-inset-bottom, 0px));
		z-index: var(--z-modal, 200);
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-2);
		border: 1px solid var(--border);
		border-radius: var(--radius-lg, 1rem);
		background: var(--bg-elevated);
		box-shadow: var(--shadow-lg, 0 8px 24px rgb(0 0 0 / 0.25));
		min-height: 4rem;
	}
	.stop {
		display: flex;
		flex: 1;
		min-height: 3.5rem;
		font-size: var(--fs-lg);
	}
	.dot {
		width: 0.875rem;
		height: 0.875rem;
		border-radius: 50%;
		background: currentColor;
		animation: pulse 1.2s ease-in-out infinite;
	}
	.stop-label {
		flex: 1;
		text-align: start;
	}
	.timer {
		font-variant-numeric: tabular-nums;
		opacity: 0.85;
	}
	@keyframes pulse {
		50% {
			opacity: 0.3;
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.dot {
			animation: none;
		}
	}
	@media (min-width: 48rem) {
		.recording {
			inset-inline: auto;
			right: var(--sp-4);
			width: min(28rem, calc(100vw - 2 * var(--sp-4)));
		}
	}
</style>
