<script lang="ts">
	// A user turn spoken in live voice: the mic, then what was heard.
	import MessageBubble from './MessageBubble.svelte';
	import { m } from '$lib/paraglide/messages';

	let {
		html,
		text,
		tinted = false,
		pending = false,
		queued = false,
		failed = false,
		cancelled = false
	}: {
		html?: string;
		text?: string;
		tinted?: boolean;
		pending?: boolean;
		queued?: boolean;
		failed?: boolean;
		cancelled?: boolean;
	} = $props();
</script>

<div class="voice-message" data-testid="voice-message">
	<span class="mic" role="img" aria-label={m.voice_user_spoken()} title={m.voice_user_spoken()}>🎙</span>
	<div class="body">
		<MessageBubble role="user" {html} {text} {tinted} {pending} {queued} {failed} {cancelled} />
	</div>
</div>

<style>
	.voice-message {
		display: flex;
		align-items: flex-start;
		gap: var(--sp-2);
	}
	.mic {
		flex: 0 0 auto;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 1.75rem;
		height: 1.75rem;
		margin-top: var(--sp-1);
		border-radius: 50%;
		border: 1px solid var(--role-user);
		background: color-mix(in srgb, var(--role-user) 14%, var(--bg-elevated));
		font-size: var(--fs-sm);
		line-height: 1;
	}
	.body {
		flex: 1;
		min-width: 0;
	}
</style>
