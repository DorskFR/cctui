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

	it('starts the first sentence while later ones render, then plays them in order', () => {
		const { p, made } = player();
		const line = (id: string, size: number) => {
			p.begin(id);
			p.chunk(new Uint8Array(size));
			p.end(id);
		};
		line('1-1', 1);
		expect(made.map((a) => a.src)).toEqual(['blob:1']);
		line('1-2', 2);
		line('1-3', 3);
		expect(made).toHaveLength(1);
		made[0].onended?.();
		made[1].onerror?.();
		made[2].onended?.();
		expect(made.map((a) => a.src)).toEqual(['blob:1', 'blob:2', 'blob:3']);
		expect(p.playing).toBe(false);
	});

	it('stop drops every queued sentence and the one still rendering', () => {
		const { p, made, idle } = player();
		for (const id of ['1-1', '1-2', '1-3']) {
			p.begin(id);
			p.chunk(new Uint8Array([1]));
			p.end(id);
		}
		p.begin('1-4');
		p.chunk(new Uint8Array([1]));
		p.stop();
		p.chunk(new Uint8Array([2]));
		p.end('1-4');
		made[0].onended?.();
		expect(made).toHaveLength(1);
		expect(p.playing).toBe(false);
		expect(idle.at(-1)).toBe(true);
	});

	it('plays the first line after an unlock', async () => {
		const { p, made } = player();
		await p.unlock();
		expect(p.playing).toBe(false);
		p.begin('a');
		p.chunk(new Uint8Array([1]));
		p.end('a');
		expect(p.playing).toBe(true);
		expect(made.at(-1)?.src).toBe('blob:1');
		expect(made.at(-1)?.play).toHaveBeenCalled();
	});
});
