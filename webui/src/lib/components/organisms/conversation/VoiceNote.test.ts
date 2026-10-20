// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

const voice = vi.hoisted(() => ({ speed: 1.5, autoPlayVoiceNotes: false }));
vi.mock('$lib/settings.svelte', () => ({ settings: { voice } }));
vi.mock('$lib/notify.svelte', () => ({ notify: { supported: false } }));

import VoiceNote from './VoiceNote.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

describe('VoiceNote playback speed', () => {
	it('applies the voice speed setting when the play button is pressed', () => {
		const play = vi.fn(async () => {});
		Object.defineProperty(HTMLMediaElement.prototype, 'play', { value: play, configurable: true });
		comp = mount(VoiceNote, {
			target: document.body,
			props: { sessionId: 's1', noteId: 'n1', text: 'hi', ts: 0 }
		});
		flushSync();
		const audio = document.querySelector('audio') as HTMLAudioElement;
		audio.playbackRate = 1;
		document.querySelector<HTMLButtonElement>('button.play')!.click();
		flushSync();
		expect(play).toHaveBeenCalledTimes(1);
		expect(audio.playbackRate).toBe(1.5);
	});
});
