import { describe, expect, it, vi } from 'vitest';
import { type AudioLike, SpeechPlayer } from './player';

class FakeAudio implements AudioLike {
	src = '';
	playbackRate = 1;
	onended: (() => void) | null = null;
	onerror: (() => void) | null = null;
	paused = false;
	play = vi.fn(async () => {});
	pause() {
		this.paused = true;
	}
}

function player() {
	const made: FakeAudio[] = [];
	const idle: boolean[] = [];
	const p = new SpeechPlayer({
		mime: () => 'audio/ogg',
		speed: () => 1.5,
		onIdle: (v) => idle.push(v),
		createAudio: () => {
			const a = new FakeAudio();
			made.push(a);
			return a;
		},
		createUrl: (blob) => `blob:${blob.size}`,
		revokeUrl: () => {}
	});
	return { p, made, idle };
}

describe('SpeechPlayer', () => {
	it('plays finished lines back to back at the chosen speed', () => {
		const { p, made, idle } = player();
		p.begin('a');
		p.chunk(new Uint8Array([1, 2]));
		p.chunk(new Uint8Array([3]));
		p.end('a');
		expect(p.playing).toBe(true);
		expect(made).toHaveLength(1);
		expect(made[0].src).toBe('blob:3');
		expect(made[0].playbackRate).toBe(1.5);
		expect(idle).toEqual([false]);

		p.begin('b');
		p.chunk(new Uint8Array([9]));
		p.end('b');
		expect(made).toHaveLength(1);
		made[0].onended?.();
		expect(made).toHaveLength(2);
		expect(made[1].src).toBe('blob:1');
		made[1].onended?.();
		expect(p.playing).toBe(false);
		expect(idle).toEqual([false, false, true]);
	});

	it('ignores a mismatched or empty line and stops everything on stop', () => {
		const { p, made } = player();
		p.begin('a');
		p.end('zzz');
		p.begin('b');
		p.end('b');
		expect(made).toHaveLength(0);

		p.begin('c');
		p.chunk(new Uint8Array([1]));
		p.end('c');
		p.begin('d');
		p.chunk(new Uint8Array([1]));
		p.end('d');
		p.stop();
		expect(made[0].paused).toBe(true);
		expect(p.playing).toBe(false);
		made[0].onended?.();
		expect(made).toHaveLength(1);
	});
});
