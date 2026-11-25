import { describe, expect, it } from 'vitest';
import pane from '../ConversationPane.svelte?raw';
import model from './paneModel.svelte.ts?raw';
import voice from './paneVoice.svelte.ts?raw';
import composer from './ConversationComposer.svelte?raw';

describe('conversation voice entry points follow the speech capabilities', () => {
	it('gates dictation on STT, read-aloud on TTS and voice mode on both', () => {
		expect(voice).toContain('readonly caps = $derived(voiceCaps(this.#config.data));');
		expect(model).toContain('readonly voiceEnabled = $derived.by(() => this.voice.caps.stt && !this.archived);');
		expect(pane).toContain('onspeak={voice.caps.tts ? voice.speak : undefined}');
		expect(pane).toContain('onvoicelive={voice.caps.live ? voice.toggleLive : undefined}');
		expect(voice).not.toContain('.data?.enabled');
	});

	it('drops an active voice mode once the instance loses a half', () => {
		expect(voice).toContain('if (!this.caps.live) untrack(() => this.live.stop());');
	});

	it('renders the shared mic only with STT and hands it voice mode only when offered', () => {
		expect(composer).toContain('{#if voiceEnabled}');
		expect(composer).toContain(
			'<VoiceControls live={voiceLive} ontogglelive={onvoicelive} ondictated={dictated} />'
		);
	});

	it('routes an auto-sent dictation like Send, so a cold session is not sent to in place', () => {
		const body = composer.slice(composer.indexOf('async function dictated('));
		const fn = body.slice(0, body.indexOf('\n\t}\n'));
		expect(fn).toContain("if (settings.voice.autoSend && route === 'send') return sendText(text, true);");
		expect(fn).toContain('if (settings.voice.autoSend) submit();');
	});

	it('sends a draft that went through the mic as dictated', () => {
		expect(composer).toContain('dictatedDraft = true;');
		expect(composer).toContain('onsend(text, dictatedDraft, followup);');
		expect(pane).toContain(
			'onsend={(body, dictated, followup) => stream.sendBody(body, dictated, followup)}'
		);
	});
});
