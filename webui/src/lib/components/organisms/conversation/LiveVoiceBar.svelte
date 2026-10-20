<script lang="ts">
	// The live voice strip above the composer: what the loop is doing, the
	// mic level, the last thing it heard, mute and leave.
	import { Badge, IconButton, Meter, Text } from '@dorsk/tsumikit';
	import type { LiveVoice } from '$lib/voice/liveVoice.svelte';
	import { m } from '$lib/paraglide/messages';

	let { voice, onstop }: { voice: LiveVoice; onstop: () => void } = $props();

	const label = $derived.by(() => {
		switch (voice.status) {
			case 'listening':
				return m.voice_live_listening();
			case 'thinking':
				return m.voice_live_thinking();
			case 'speaking':
				return m.voice_live_speaking();
			default:
				return m.voice_live_connecting();
		}
	});
	const color = $derived(
		voice.status === 'speaking'
			? 'var(--role-assistant)'
			: voice.status === 'thinking'
				? 'var(--warn)'
				: 'var(--ok)'
	);
</script>

<div class="live-bar" class:muted={voice.muted} data-journey="voice-live" role="status">
	<Badge size="xs" uppercase {color}>{label}</Badge>
	<div class="meter">
		<Meter label={m.voice_live_level()} value={voice.muted ? 0 : voice.level} max={1} size="sm" format={() => ''} />
	</div>
	{#if voice.transcript}
		<span class="transcript"><Text size="sm" tone="muted">🎙 {voice.transcript}</Text></span>
	{/if}
	<IconButton
		emoji={voice.muted ? '🔇' : '🎙'}
		box="sm"
		variant="ghost"
		pressed={voice.muted}
		label={voice.muted ? m.voice_live_unmute() : m.voice_live_mute()}
		title={voice.muted ? m.voice_live_unmute() : m.voice_live_mute()}
		onclick={() => voice.toggleMute()}
	/>
	{#if voice.status === 'speaking'}
		<IconButton
			icon="pause"
			box="sm"
			variant="ghost"
			label={m.voice_live_interrupt()}
			title={m.voice_live_interrupt()}
			onclick={() => voice.interrupt()}
		/>
	{/if}
	<IconButton
		icon="stop"
		box="sm"
		variant="danger"
		label={m.voice_live_stop()}
		title={m.voice_live_stop()}
		onclick={onstop}
	/>
</div>

<style>
	.live-bar {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-1) var(--sp-3);
		border-top: 1px solid var(--border);
		background: var(--bg-elevated);
	}
	.meter {
		width: 6rem;
		flex: 0 0 auto;
	}
	.transcript {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.live-bar.muted .meter {
		opacity: 0.4;
	}
</style>
