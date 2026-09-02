import { describe, expect, it } from 'vitest';
import { appendFileTokens, extForType, makeClipboardFiles } from './attachments';

const f = (name: string) => new File(['x'], name, { type: 'text/plain' });

describe('appendFileTokens', () => {
	it('appends a bracketed token per file to empty text', () => {
		expect(appendFileTokens('', [f('a.png'), f('b.csv')])).toBe('[a.png] [b.csv]');
	});

	it('separates from existing text with a space', () => {
		expect(appendFileTokens('see this:', [f('a.png')])).toBe('see this: [a.png]');
	});

	it('does not double a trailing space or newline', () => {
		expect(appendFileTokens('line one\n', [f('a.png')])).toBe('line one\n[a.png]');
		expect(appendFileTokens('word ', [f('a.png')])).toBe('word [a.png]');
	});

	it('skips names already referenced in the text', () => {
		expect(appendFileTokens('about [a.png] here', [f('a.png'), f('b.csv')])).toBe(
			'about [a.png] here [b.csv]'
		);
	});

	it('is idempotent on a re-pick of the same file', () => {
		const once = appendFileTokens('', [f('a.png')]);
		expect(appendFileTokens(once, [f('a.png')])).toBe(once);
	});
});

describe('extForType', () => {
	it('maps known MIME types', () => {
		expect(extForType('image/png')).toBe('png');
		expect(extForType('application/pdf')).toBe('pdf');
	});
	it('falls back to the sanitised subtype, then bin', () => {
		expect(extForType('text/x-log; charset=utf-8')).toBe('xlog');
		expect(extForType('')).toBe('bin');
	});
});

describe('makeClipboardFiles', () => {
	const item = (file: File | null, kind = 'file') =>
		({ kind, getAsFile: () => file }) as unknown as DataTransferItem;
	const dt = (items: DataTransferItem[], files: File[] = []) =>
		({ items, files }) as unknown as DataTransfer;

	it('names nameless blobs uniquely per surface', () => {
		const fromClipboard = makeClipboardFiles();
		const blob = new File(['x'], '', { type: 'image/png' });
		const a = fromClipboard(dt([item(blob)]));
		const b = fromClipboard(dt([item(blob)]));
		expect(a.map((f) => f.name)).toEqual(['clipboard-1.png']);
		expect(b.map((f) => f.name)).toEqual(['clipboard-2.png']);
	});

	it('keeps named files and ignores string items', () => {
		const fromClipboard = makeClipboardFiles();
		const named = new File(['x'], 'shot.png', { type: 'image/png' });
		const out = fromClipboard(dt([item(null, 'string'), item(named)]));
		expect(out).toEqual([named]);
	});

	it('falls back to .files when items carry no file', () => {
		const fromClipboard = makeClipboardFiles();
		const f = new File(['x'], 'a.txt', { type: 'text/plain' });
		expect(fromClipboard(dt([item(null, 'string')], [f]))).toEqual([f]);
	});
});
