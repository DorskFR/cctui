// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import VoiceControls from './VoiceControls.svelte';
import { HOLD_MS, MIN_CLIP_MS } from '$lib/voice/tapHold';

const { postForm, toast } = vi.hoisted(() => ({
	postForm: vi.fn(async () => ({ text: 'hello there' })),
	toast: { error: vi.fn(), info: vi.fn() }
}));
vi.mock('$lib/api', () => ({ api: { postForm }, errMessage: (e: unknown) => String(e) }));
vi.mock('$lib/toast.svelte', () => ({ toasts: toast }));

class FakeRecorder {
	state = 'inactive';
	mimeType = 'audio/webm';
	ondataavailable: ((e: { data: Blob }) => void) | null = null;
	onstop: (() => void) | null = null;
	static isTypeSupported = () => true;
	start() {
		this.state = 'recording';
	}
	stop() {
		this.state = 'inactive';
		this.ondataavailable?.({ data: new Blob(['x'.repeat(100)]) });
		this.onstop?.();
	}
}

let comp: ReturnType<typeof mount> | null = null;
const ondictated = vi.fn();
const NOW = Date.parse('2026-10-23T10:00:00Z');

beforeEach(() => {
	vi.useFakeTimers();
	vi.setSystemTime(NOW);
	vi.stubGlobal('navigator', {
		mediaDevices: { getUserMedia: async () => ({ getTracks: () => [{ stop: vi.fn() }] }) }
	});
	vi.stubGlobal('MediaRecorder', FakeRecorder);
	postForm.mockClear();
	ondictated.mockClear();
	toast.info.mockClear();
	comp = mount(VoiceControls, {
		target: document.body,
		props: { live: false, ontogglelive: vi.fn(), ondictated }
	});
	flushSync();
});

afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	vi.useRealTimers();
	vi.unstubAllGlobals();
});

const mic = () => document.querySelector<HTMLButtonElement>('button[aria-label^="Dictate"]')!;
const recordingPanel = () => document.querySelector('[data-journey="voice-recording"]');
const fire = (el: Element, type: string) => {
	el.dispatchEvent(new Event(type, { bubbles: true, cancelable: true }));
	flushSync();
};
const settle = async () => {
	for (let i = 0; i < 6; i++) await Promise.resolve();
	flushSync();
};
const press = async (ms: number) => {
	fire(mic(), 'pointerdown');
	await settle();
	vi.advanceTimersByTime(ms);
	fire(mic(), 'pointerup');
	await settle();
};

describe('VoiceControls dictation gestures', () => {
	it('a tap starts recording and shows the large stop control', async () => {
		await press(80);
		expect(recordingPanel()).not.toBeNull();
		expect(mic().getAttribute('aria-pressed')).toBe('true');
		expect(document.querySelector('button[aria-label="Stop and send"]')).not.toBeNull();
		expect(postForm).not.toHaveBeenCalled();
	});

	it('a second tap stops, transcribes and hands the text over', async () => {
		await press(80);
		vi.advanceTimersByTime(5000);
		await press(80);
		expect(recordingPanel()).toBeNull();
		expect(postForm).toHaveBeenCalledTimes(1);
		expect(ondictated).toHaveBeenCalledWith('hello there');
	});

	it('the large stop control stops and sends too, and shows a running timer', async () => {
		await press(80);
		vi.advanceTimersByTime(61_250);
		flushSync();
		expect(recordingPanel()?.textContent).toContain('1:01');
		fire(document.querySelector('button[aria-label="Stop and send"]')!, 'click');
		await settle();
		expect(postForm).toHaveBeenCalledTimes(1);
		expect(ondictated).toHaveBeenCalledWith('hello there');
	});

	it('a hold past the threshold sends on release', async () => {
		await press(HOLD_MS + 2000);
		expect(recordingPanel()).toBeNull();
		expect(postForm).toHaveBeenCalledTimes(1);
		expect(ondictated).toHaveBeenCalledWith('hello there');
	});

	it('a quick double tap sends nothing and shows no empty-clip notice', async () => {
		await press(40);
		vi.advanceTimersByTime(MIN_CLIP_MS - 200);
		await press(40);
		expect(recordingPanel()).toBeNull();
		expect(postForm).not.toHaveBeenCalled();
		expect(ondictated).not.toHaveBeenCalled();
		expect(toast.info).not.toHaveBeenCalled();
	});

	it('discard drops the recording without transcribing', async () => {
		await press(80);
		vi.advanceTimersByTime(3000);
		fire(document.querySelector('button[aria-label="Discard recording"]')!, 'click');
		await settle();
		expect(recordingPanel()).toBeNull();
		expect(postForm).not.toHaveBeenCalled();
	});

	it('a long press on the mic opens no context menu', () => {
		const e = new Event('contextmenu', { bubbles: true, cancelable: true });
		mic().dispatchEvent(e);
		expect(e.defaultPrevented).toBe(true);
	});

	it('a tap that lingers short of the hold time keeps recording until the next tap', async () => {
		await press(HOLD_MS - 50);
		expect(recordingPanel()).not.toBeNull();
		vi.advanceTimersByTime(30_000);
		await press(80);
		expect(recordingPanel()).toBeNull();
		expect(postForm).toHaveBeenCalledTimes(1);
		expect(ondictated).toHaveBeenCalledWith('hello there');
	});

	it('the keyboard toggles recording', async () => {
		mic().dispatchEvent(new KeyboardEvent('keydown', { key: ' ', bubbles: true, cancelable: true }));
		await settle();
		expect(recordingPanel()).not.toBeNull();
		vi.advanceTimersByTime(2000);
		mic().dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }));
		await settle();
		expect(recordingPanel()).toBeNull();
		expect(ondictated).toHaveBeenCalledWith('hello there');
	});
});

describe('VoiceControls without voice mode', () => {
	it('shows the mic but no voice-mode toggle when no live handler is given', () => {
		const toggle = () => document.querySelector('[aria-label="Live voice conversation with this session"]');
		expect(toggle()).not.toBeNull();
		if (comp) unmount(comp);
		document.body.innerHTML = '';
		comp = mount(VoiceControls, { target: document.body, props: { ondictated } });
		flushSync();
		expect(mic()).not.toBeNull();
		expect(toggle()).toBeNull();
	});
});
