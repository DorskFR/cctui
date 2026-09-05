import { describe, expect, it } from 'vitest';
import { classify, refusalMessage } from './fileviewer';

describe('fileviewer classify', () => {
	it('routes by base content type', () => {
		expect(classify('image/png')).toBe('image');
		expect(classify('text/markdown; charset=utf-8')).toBe('markdown');
		expect(classify('text/plain; charset=utf-8')).toBe('text');
		expect(classify('application/json; charset=utf-8')).toBe('text');
		expect(classify('application/pdf')).toBe('download');
		expect(classify('application/octet-stream')).toBe('download');
		expect(classify('text/html')).toBe('download');
		expect(classify(null)).toBe('download');
	});
});

describe('fileviewer refusalMessage', () => {
	it('names the file and distinguishes the refusal kinds', () => {
		const tooLarge = refusalMessage(413, 'big.zip');
		const denied = refusalMessage(403, 'x.md');
		const missing = refusalMessage(404, 'x.md');
		const offline = refusalMessage(503, 'x.md');
		const other = refusalMessage(500, 'x.md');
		for (const t of [tooLarge, denied, missing, offline, other]) expect(t).toMatch(/x\.md|big\.zip/);
		expect(new Set([tooLarge, denied, missing, offline, other]).size).toBe(5);
		expect(other).toContain('500');
	});
});
