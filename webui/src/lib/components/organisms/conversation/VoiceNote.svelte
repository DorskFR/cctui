<script lang="ts">
	import { onMount } from 'svelte';
	import { settings } from '$lib/settings.svelte';
	import { notify } from '$lib/notify.svelte';
	import { m } from '$lib/paraglide/messages';
	import { claimAnnounce, formatClock, voiceNoteUrl } from './voiceNote';

	let {
		sessionId,
		noteId,
		text,
		ts,
		sessionLabel = ''
	}: {
		sessionId: string;
		noteId: string;
		text: string;
		ts: number;
		sessionLabel?: string;
	} = $props();

	let audio: HTMLAudioElement | undefined = $state();
	let playing = $state(false);
	let current = $state(0);
	let duration = $state(0);

	const progress = $derived(duration > 0 ? Math.min(1, current / duration) : 0);

	function toggle() {
		if (!audio) return;
		if (audio.paused) void audio.play().catch(() => (playing = false));
		else audio.pause();
	}

	function seek(e: MouseEvent) {
		if (!audio || !duration) return;
		const box = (e.currentTarget as HTMLElement).getBoundingClientRect();
		audio.currentTime = ((e.clientX - box.left) / box.width) * duration;
	}

	function announce() {
		if (!notify.supported || Notification.permission !== 'granted') return;
		try {
			new Notification(m.voice_note_notification({ session: sessionLabel || sessionId.slice(0, 8) }), {
				body: text,
				tag: `voice-${noteId}`
			});
		} catch {
			/* best effort */
		}
	}

	onMount(() => {
		if (!claimAnnounce(noteId, ts)) return;
		if (settings.voice.autoPlayVoiceNotes && audio && !document.hidden) {
			audio.playbackRate = settings.voice.speed;
			void audio.play().catch(announce);
		} else {
			announce();
		}
	});
</script>

<div class="voice-note" data-testid="voice-note">
	<audio
		bind:this={audio}
		src={voiceNoteUrl(sessionId, noteId)}
		preload="metadata"
		bind:currentTime={current}
		bind:duration
		onplay={() => (playing = true)}
		onpause={() => (playing = false)}
		onended={() => (playing = false)}
	></audio>
	<div class="player">
		<button
			type="button"
			class="play"
			aria-label={playing ? m.voice_note_pause() : m.voice_note_play()}
			onclick={toggle}>{playing ? '⏸' : '▶'}</button
		>
		<!-- svelte-ignore a11y_click_events_have_key_events -->
		<div class="track" role="presentation" onclick={seek}>
			<div class="fill" style:width="{progress * 100}%"></div>
		</div>
		<span class="clock">{formatClock(current)} / {formatClock(duration)}</span>
	</div>
	<details class="transcript">
		<summary>{m.voice_note_transcript()}</summary>
		<p>{text}</p>
	</details>
</div>

<style>
	.voice-note {
		display: flex;
		flex-direction: column;
		gap: 4px;
		padding: 8px 10px;
		border: 1px solid var(--border);
		border-radius: 8px;
		max-width: 28rem;
	}
	.player {
		display: flex;
		align-items: center;
		gap: 8px;
	}
	.play {
		width: 28px;
		height: 28px;
		border-radius: 50%;
		border: 1px solid var(--border);
		background: transparent;
		color: inherit;
		cursor: pointer;
	}
	.track {
		flex: 1;
		height: 4px;
		border-radius: 2px;
		background: var(--border);
		cursor: pointer;
	}
	.fill {
		height: 100%;
		border-radius: 2px;
		background: var(--role-assistant);
	}
	.clock {
		font-variant-numeric: tabular-nums;
		font-size: 0.8em;
		color: var(--text-muted);
	}
	.transcript summary {
		cursor: pointer;
		font-size: 0.8em;
		color: var(--text-muted);
	}
	.transcript p {
		margin: 4px 0 0;
		white-space: pre-wrap;
	}
</style>
