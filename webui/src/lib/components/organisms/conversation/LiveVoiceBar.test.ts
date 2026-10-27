// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import LiveVoiceBar from './LiveVoiceBar.svelte';
import type { LiveVoice, LiveStatus } from '$lib/voice/liveVoice.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const render = (status: LiveStatus, over: Partial<LiveVoice> = {}, landed = false) => {
	const voice = {
		status,
		level: 0,
		muted: false,
		transcript: '',
		language: 'auto',
		setLanguage: vi.fn(),
		toggleMute: vi.fn(),
		interrupt: vi.fn(),
		...over
	} as unknown as LiveVoice;
	comp = mount(LiveVoiceBar, { target: document.body, props: { voice, onstop: vi.fn(), landed } });
	flushSync();
	return voice;
};

const text = () => document.body.textContent ?? '';

describe('LiveVoiceBar', () => {
	it('shows the transcript until its voice message lands in the conversation', () => {
		render('thinking', { transcript: 'what is two plus two' });
		expect(text()).toContain('what is two plus two');
		if (comp) unmount(comp);
		document.body.innerHTML = '';
		render('thinking', { transcript: 'what is two plus two' }, true);
		expect(text()).not.toContain('what is two plus two');
	});

	it('says it is warming up while the model loads', () => {
		render('warming');
		expect(text()).toContain('Warming up…');
	});

	it('shows listening once warm', () => {
		render('listening');
		expect(text()).toContain('Listening');
		expect(text()).not.toContain('Warming up');
	});

	it('leaves the spoken language to Settings → Voice', () => {
		render('listening');
		expect(document.querySelector('select')).toBeNull();
	});
});
