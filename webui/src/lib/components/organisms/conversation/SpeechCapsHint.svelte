<script module lang="ts">
	const hint = $state({ dismissed: false });
</script>

<script lang="ts">
	import { IconButton, Text } from '@dorsk/tsumikit';
	import { useMe, useVoiceConfig } from '#lib/queries/index.ts';
	import { missingVoiceHalves } from '#lib/components/organisms/settings/speech.logic.ts';
	import { settingsHref } from '#lib/components/organisms/settings/settings.logic.ts';
	import { m } from '#lib/paraglide/messages.js';

	const me = useMe();
	const voiceConfig = useVoiceConfig();
	const missing = $derived(me.data?.role === 'admin' ? missingVoiceHalves(voiceConfig.data) : []);
</script>

{#if missing.length && !hint.dismissed}
	<div class="speech-hint" data-testid="speech-caps-hint">
		<Text tone="muted" size="sm">
			{missing.includes('stt') ? m.composer_speech_hint_stt() : m.composer_speech_hint_tts()}
			<a href="{settingsHref('instance')}#speech">{m.composer_speech_hint_open()}</a>
		</Text>
		<IconButton
			icon="x"
			variant="ghost"
			box="xs"
			label={m.composer_speech_hint_dismiss()}
			onclick={() => (hint.dismissed = true)}
		/>
	</div>
{/if}

<style>
	.speech-hint {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
		padding: var(--sp-1) var(--sp-2);
	}
</style>
