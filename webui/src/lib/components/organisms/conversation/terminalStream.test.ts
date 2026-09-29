// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest';
import { PtyStream, loadTerminalFont } from './terminalStream.svelte';

const bytes = (s: string) => new TextEncoder().encode(s);

function writer() {
	const written: Uint8Array[] = [];
	return { written, write: (d: Uint8Array) => written.push(d) };
}

describe('PtyStream', () => {
	it('writes chunks that arrived before the terminal opened, in order', () => {
		const s = new PtyStream();
		s.push(bytes('a'));
		s.push(bytes('b'));
		const w = writer();
		expect(w.written).toEqual([]);
		s.attach(w);
		expect(w.written).toEqual([bytes('a'), bytes('b')]);
	});

	it('writes straight through once attached, buffering nothing', () => {
		const s = new PtyStream();
		const w = writer();
		s.attach(w);
		s.push(bytes('a'));
		expect(w.written).toEqual([bytes('a')]);
		const next = writer();
		s.attach(next);
		expect(next.written).toEqual([]);
	});

	it('is not live until the first chunk arrives', () => {
		const s = new PtyStream();
		expect(s.live).toBe(false);
		s.attach(writer());
		expect(s.live).toBe(false);
		s.push(bytes('a'));
		expect(s.live).toBe(true);
	});

	it('drops the buffer and goes offline on detach', () => {
		const s = new PtyStream();
		s.push(bytes('a'));
		s.detach();
		expect(s.live).toBe(false);
		const w = writer();
		s.attach(w);
		expect(w.written).toEqual([]);
	});
});

describe('loadTerminalFont', () => {
	it('resolves via the cap when the font never loads', async () => {
		vi.useFakeTimers();
		const fonts = { load: () => new Promise(() => {}) };
		vi.stubGlobal('document', { fonts });
		const done = vi.fn();
		void loadTerminalFont('JetBrains Mono', 500).then(done);
		await vi.advanceTimersByTimeAsync(500);
		expect(done).toHaveBeenCalled();
		vi.useRealTimers();
		vi.unstubAllGlobals();
	});

	it('resolves rather than throwing when the fonts API rejects', async () => {
		const fonts = { load: () => Promise.reject(new Error('nope')) };
		vi.stubGlobal('document', { fonts });
		await expect(loadTerminalFont('JetBrains Mono', 500)).resolves.toBeUndefined();
		vi.unstubAllGlobals();
	});
});
