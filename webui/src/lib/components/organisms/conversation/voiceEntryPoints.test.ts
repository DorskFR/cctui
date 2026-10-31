import { describe, expect, it } from 'vitest';
import pane from '../ConversationPane.svelte?raw';
import composer from './ConversationComposer.svelte?raw';

describe('conversation voice entry points follow the speech capabilities', () => {
	it('gates dictation on STT, read-aloud on TTS and voice mode on both', () => {
		expect(pane).toContain('const caps = $derived(voiceCaps(voiceConfig.data));');
		expect(pane).toContain('const voiceEnabled = $derived(caps.stt && !archived);');
		expect(pane).toContain('onspeak={caps.tts ? speakLine : undefined}');
		expect(pane).toContain('onvoicelive={caps.live ? toggleLiveVoice : undefined}');
		expect(pane).not.toContain('voiceConfig.data?.enabled');
	});

	it('drops an active voice mode once the instance loses a half', () => {
		expect(pane).toContain('if (!caps.live) untrack(() => liveVoice.stop());');
	});

	it('renders the shared mic only with STT and hands it voice mode only when offered', () => {
		expect(composer).toContain('{#if voiceEnabled}');
		expect(composer).toContain(
			'<VoiceControls live={voiceLive} ontogglelive={onvoicelive} ondictated={dictated} />'
		);
	});
});
