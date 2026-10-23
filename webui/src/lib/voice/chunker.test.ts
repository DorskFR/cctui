import { describe, expect, it } from 'vitest';
import { UtteranceChunker } from './chunker';

const opts = { sampleRate: 1000, minPartMs: 100, pauseMs: 20, maxPartMs: 300, preRollMs: 50 };

function fed(): { emitted: { samples: Float32Array; last: boolean }[]; c: UtteranceChunker } {
	const emitted: { samples: Float32Array; last: boolean }[] = [];
	const c = new UtteranceChunker((samples, last) => emitted.push({ samples, last }), opts);
	return { emitted, c };
}

/** `n` samples numbered from `from`, so order and loss are visible. */
function ramp(from: number, n: number): Float32Array {
	return Float32Array.from({ length: n }, (_, i) => (from + i) / 1e6);
}

function joined(parts: Float32Array[]): number[] {
	return parts.flatMap((p) => Array.from(p));
}

describe('UtteranceChunker', () => {
	it('sends a short utterance whole, pre-roll included, as the final part', () => {
		const { emitted, c } = fed();
		c.frame(ramp(0, 10), 0.1);
		c.start();
		c.frame(ramp(10, 20), 0.9);
		c.frame(ramp(30, 5), 0.2);
		c.end();
		expect(emitted.map((e) => e.last)).toEqual([true]);
		expect(joined([emitted[0].samples])).toEqual(Array.from(ramp(0, 35)));
	});

	it('keeps only the last pre-roll frames', () => {
		const { emitted, c } = fed();
		for (let i = 0; i < 10; i++) c.frame(ramp(i * 20, 20), 0.1);
		c.start();
		c.end();
		expect(emitted[0].samples.length).toBeLessThanOrEqual(60);
		expect(emitted[0].samples.at(-1)).toBe(Math.fround(199 / 1e6));
	});

	it('flushes a part at a pause once enough is buffered', () => {
		const { emitted, c } = fed();
		c.start();
		c.frame(ramp(0, 100), 0.9);
		c.frame(ramp(100, 10), 0.1);
		expect(emitted).toEqual([]);
		c.frame(ramp(110, 10), 0.1);
		expect(emitted.map((e) => [e.samples.length, e.last])).toEqual([[120, false]]);
		c.frame(ramp(120, 30), 0.9);
		c.end();
		expect(emitted.map((e) => [e.samples.length, e.last])).toEqual([
			[120, false],
			[30, true]
		]);
		expect(joined(emitted.map((e) => e.samples))).toEqual(Array.from(ramp(0, 150)));
	});

	it('a pause before the minimum does not flush, and speech resets the pause', () => {
		const { emitted, c } = fed();
		c.start();
		c.frame(ramp(0, 50), 0.9);
		c.frame(ramp(50, 30), 0.1);
		expect(emitted).toEqual([]);
		c.frame(ramp(80, 30), 0.9);
		c.frame(ramp(110, 10), 0.1);
		expect(emitted).toEqual([]);
	});

	it('flushes at the hard cap without a pause', () => {
		const { emitted, c } = fed();
		c.start();
		for (let i = 0; i < 7; i++) c.frame(ramp(i * 50, 50), 0.9);
		expect(emitted.map((e) => [e.samples.length, e.last])).toEqual([[300, false]]);
		c.end();
		expect(emitted.map((e) => [e.samples.length, e.last])).toEqual([
			[300, false],
			[50, true]
		]);
		expect(joined(emitted.map((e) => e.samples))).toEqual(Array.from(ramp(0, 350)));
	});

	it('ends with an empty final part when speech stops right after a flush', () => {
		const { emitted, c } = fed();
		c.start();
		for (let i = 0; i < 6; i++) c.frame(ramp(i * 50, 50), 0.9);
		c.end();
		expect(emitted.map((e) => [e.samples.length, e.last])).toEqual([
			[300, false],
			[0, true]
		]);
	});

	it('a misfire with nothing sent is discarded and does not leak into the next utterance', () => {
		const { emitted, c } = fed();
		c.frame(ramp(0, 10), 0.1);
		c.start();
		c.frame(ramp(10, 20), 0.9);
		c.cancel();
		expect(emitted).toEqual([]);
		c.start();
		c.frame(ramp(100, 5), 0.9);
		c.end();
		expect(joined(emitted.map((e) => e.samples))).toEqual(Array.from(ramp(100, 5)));
	});

	it('a cancel after a part went out closes the utterance', () => {
		const { emitted, c } = fed();
		c.start();
		for (let i = 0; i < 6; i++) c.frame(ramp(i * 50, 50), 0.9);
		c.frame(ramp(300, 20), 0.9);
		c.cancel();
		expect(emitted.map((e) => [e.samples.length, e.last])).toEqual([
			[300, false],
			[0, true]
		]);
	});

	it('end and cancel outside speech do nothing', () => {
		const { emitted, c } = fed();
		c.end();
		c.cancel();
		c.frame(ramp(0, 10), 0.9);
		c.end();
		expect(emitted).toEqual([]);
	});

	it('loses and repeats no sample over a long mixed utterance', () => {
		const { emitted, c } = fed();
		let at = 0;
		const feed = (n: number, prob: number) => {
			c.frame(ramp(at, n), prob);
			at += n;
		};
		feed(30, 0.1);
		c.start();
		for (let i = 0; i < 40; i++) feed(i % 7 === 0 ? 25 : 40, i % 5 === 0 ? 0.05 : 0.8);
		c.end();
		expect(emitted.length).toBeGreaterThan(2);
		expect(emitted.slice(0, -1).every((e) => !e.last && e.samples.length <= 300)).toBe(true);
		expect(emitted.at(-1)?.last).toBe(true);
		expect(joined(emitted.map((e) => e.samples))).toEqual(Array.from(ramp(0, at)));
	});
});
