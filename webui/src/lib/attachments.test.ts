import { describe, expect, it } from 'vitest';
import {
	appendFileTokens,
	attachFiles,
	clipLegend,
	expandClipTokens,
	insertTokens,
	renumberClipTokens,
	DEFAULT_UPLOAD_CAPS,
	extForType,
	fileCapError,
	makeClipboardFiles,
	mergeFiles,
	mergeFilesRenamed,
	nextPasteIndex,
	maskedPaste,
	PASTE_MASK_CHARS,
	prefixImageTokens,
	rewriteFileTokens
} from './attachments';

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

describe('mergeFiles', () => {
	it('keeps distinct names in order', () => {
		expect(mergeFiles([f('a.txt')], [f('b.txt')]).map((x) => x.name)).toEqual(['a.txt', 'b.txt']);
	});

	it('renames a duplicate instead of replacing it', () => {
		const { files, added } = mergeFilesRenamed([f('a.txt')], [f('a.txt')]);
		expect(files.map((x) => x.name)).toEqual(['a.txt', 'a-2.txt']);
		expect(added.map((x) => x.name)).toEqual(['a-2.txt']);
	});

	it('skips suffixes already taken, including within one batch', () => {
		const out = mergeFiles([f('a.txt'), f('a-2.txt')], [f('a.txt'), f('a.txt'), f('noext')]);
		expect(out.map((x) => x.name)).toEqual(['a.txt', 'a-2.txt', 'a-3.txt', 'a-4.txt', 'noext']);
		expect(mergeFiles([f('noext')], [f('noext')]).map((x) => x.name)).toEqual(['noext', 'noext-2']);
	});
});

describe('attachFiles', () => {
	it('rewrites the token to the renamed file', () => {
		const { files, text } = attachFiles([f('a.txt')], '[a.txt]', [f('a.txt')], 'name');
		expect(files.map((x) => x.name)).toEqual(['a.txt', 'a-2.txt']);
		expect(text).toBe('[a.txt] [a-2.txt]');
	});

	it('leaves the text untouched when not tokenizing', () => {
		const { files, text } = attachFiles([], 'hello', [f('a.txt'), f('b.txt')], false);
		expect(files.map((x) => x.name)).toEqual(['a.txt', 'b.txt']);
		expect(text).toBe('hello');
	});
});

describe('clip tokens', () => {
	it('marks each attached file with a short numbered token at the caret', () => {
		const shots = [1, 2, 3].map((i) => f(`Screenshot 2026-10-02 at 11.3${i}.22.png`));
		const { files, text, caret } = attachFiles([], 'before after', shots, 'clip', 6);
		expect(files).toHaveLength(3);
		expect(text).toBe('before [#1] [#2] [#3] after');
		expect(caret).toBe('before [#1] [#2] [#3]'.length);
		expect(text.length).toBeLessThan(40);
	});

	it('numbers new files after the ones already attached and appends without a caret', () => {
		const { text } = attachFiles([f('a.png')], 'see [#1]', [f('b.png')]);
		expect(text).toBe('see [#1] [#2]');
	});

	it('expands each token to its file at its own position, in any order', () => {
		const files = [f('a.png'), f('b.png')];
		expect(expandClipTokens('second [#2] then first [#1]', files)).toBe(
			'second [b.png] then first [a.png]'
		);
		expect(
			expandClipTokens('[#1] and [#2]', files, ['/tmp/u/a.png', '/tmp/u/b-1.png'])
		).toBe('[a.png] and [b-1.png]');
		expect(expandClipTokens('stray [#9] here', files)).toBe('stray here');
		expect(expandClipTokens('legacy [📎2]', files)).toBe('legacy [b.png]');
	});

	it('renumbers tokens when a file is removed and drops the removed one', () => {
		const out = renumberClipTokens('a [#1] b [#2] c [#3]', ['x', 'y', 'z'], ['x', 'z']);
		expect(out).toBe('a [#1] b c [#2]');
		expect(renumberClipTokens('[#1] text', ['x'], [])).toBe('text');
	});

	it('lists what each token points at', () => {
		expect(clipLegend([f('a.png'), f('b.pdf')])).toBe('#1 a.png · #2 b.pdf');
	});
});

describe('insertTokens', () => {
	it('pads the tokens off the words around the caret', () => {
		expect(insertTokens('ab', ['[t]'], 1)).toEqual({ text: 'a [t] b', caret: 5 });
		expect(insertTokens('a ', ['[t]'], 2)).toEqual({ text: 'a [t]', caret: 5 });
		expect(insertTokens('', ['[t]', '[u]'])).toEqual({ text: '[t] [u]', caret: 7 });
	});
});

describe('maskedPaste', () => {
	it('collapses a long paste into the next paste-N.txt', async () => {
		const text = 'y'.repeat(PASTE_MASK_CHARS);
		const file = maskedPaste(text, [f('paste-1.txt')], '');
		expect(file?.name).toBe('paste-2.txt');
		expect(file?.type).toBe('text/plain');
		expect(await file?.text()).toBe(text);
	});

	it('leaves a short or empty paste alone', () => {
		expect(maskedPaste('y'.repeat(PASTE_MASK_CHARS - 1), [], '')).toBeNull();
		expect(maskedPaste('', [], '')).toBeNull();
	});
});

describe('nextPasteIndex', () => {
	const paste = (files: File[], text: string) => {
		const name = `paste-${nextPasteIndex(files, text)}.txt`;
		return attachFiles(files, text, [f(name)], 'name');
	};

	it('numbers consecutive pastes paste-1, paste-2', () => {
		const a = paste([], '');
		const b = paste(a.files, a.text);
		expect(b.files.map((x) => x.name)).toEqual(['paste-1.txt', 'paste-2.txt']);
		expect(b.text).toBe('[paste-1.txt] [paste-2.txt]');
	});

	it('continues from tokens in the draft after a remount with no attachments', () => {
		expect(nextPasteIndex([], 'notes [paste-1.txt] more')).toBe(2);
		expect(paste([], '[paste-3.txt] [paste-1.txt]').text).toBe(
			'[paste-3.txt] [paste-1.txt] [paste-4.txt]'
		);
	});

	it('ignores non-paste names', () => {
		expect(nextPasteIndex([f('clipboard-7.png'), f('mypaste-2.txt')], '')).toBe(1);
	});

	it('skips names the session already staged', () => {
		expect(nextPasteIndex([], '', ['paste-1.txt'])).toBe(2);
		expect(nextPasteIndex([], '', ['paste-1.txt', 'paste-1-1.txt', 'shot.png'])).toBe(2);
		expect(nextPasteIndex([f('paste-4.txt')], '', ['paste-2.txt'])).toBe(5);
	});
});

describe('rewriteFileTokens', () => {
	it('points a token at the name staging returned', () => {
		const text = 'look\n\n[paste-1.txt]\n\nplease';
		const out = rewriteFileTokens(text, [f('paste-1.txt')], [
			'/tmp/cctui-uploads/s/paste-1-1.txt'
		]);
		expect(out).toBe('look\n\n[paste-1-1.txt]\n\nplease');
	});

	it('rewrites every occurrence and leaves unrenamed files alone', () => {
		const files = [f('paste-1.txt'), f('shot.png')];
		const paths = ['/tmp/cctui-uploads/s/paste-1-2.txt', '/tmp/cctui-uploads/s/shot.png'];
		expect(rewriteFileTokens('[paste-1.txt] a [shot.png] b [paste-1.txt]', files, paths)).toBe(
			'[paste-1-2.txt] a [shot.png] b [paste-1-2.txt]'
		);
	});

	it('leaves the text untouched when nothing was staged', () => {
		expect(rewriteFileTokens('[paste-1.txt]', [], [])).toBe('[paste-1.txt]');
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

describe('fileCapError', () => {
	const sized = (name: string, size: number) =>
		new File([new Uint8Array(size)], name, { type: 'application/octet-stream' });
	const caps = { max_files: 2, max_file_bytes: 1024, max_total_bytes: 1536 };

	it('accepts a list inside the injected caps', () => {
		expect(fileCapError([sized('a', 1024), sized('b', 512)], caps)).toBe('');
	});

	it('reports the injected per-file cap, not the built-in one', () => {
		const msg = fileCapError([sized('a', 1025)], caps);
		expect(msg).toContain('per-file cap');
		expect(msg).toContain('1 KB');
		expect(fileCapError([sized('a', 1025)])).toBe('');
	});

	it('reports the injected count cap', () => {
		expect(fileCapError([sized('a', 1), sized('b', 1), sized('c', 1)], caps)).toBe(
			'Too many files (max 2)'
		);
	});

	it('reports the injected total cap', () => {
		expect(fileCapError([sized('a', 1024), sized('b', 1024)], caps)).toContain('total cap');
	});

	it('falls back to the built-in caps when none are injected', () => {
		expect(DEFAULT_UPLOAD_CAPS).toEqual({
			max_files: 10,
			max_file_bytes: 5 * 1024 * 1024,
			max_total_bytes: 20 * 1024 * 1024
		});
		expect(fileCapError([sized('a', 5 * 1024 * 1024 + 1)])).toContain('5.0 MB per-file cap');
		expect(fileCapError(Array.from({ length: 11 }, (_, i) => sized(String(i), 1)))).toBe(
			'Too many files (max 10)'
		);
	});
});

describe('prefixImageTokens', () => {
	const img = (name: string) => new File(['x'], name, { type: 'image/png' });

	it('leads the body with the staged name of each image', () => {
		const files = [img('Screenshot 1.png'), img('b.png'), f('notes.txt')];
		const paths = ['/tmp/u/s/Screenshot 1.png', '/tmp/u/s/b-1.png', '/tmp/u/s/notes.txt'];
		expect(prefixImageTokens('look', files, paths)).toBe('[Screenshot 1.png] [b-1.png]\nlook');
		expect(prefixImageTokens('', files, paths)).toBe('[Screenshot 1.png] [b-1.png]');
	});

	it('skips names the leading run already carries, not ones later in the prose', () => {
		const files = [img('a.png'), img('b.png')];
		const paths = ['/tmp/a.png', '/tmp/b.png'];
		expect(prefixImageTokens('[a.png] see [b.png]', files, paths)).toBe('[b.png]\n[a.png] see [b.png]');
		expect(prefixImageTokens('[a.png] [b.png] hi', files, paths)).toBe('[a.png] [b.png] hi');
	});

	it('leaves a body without images untouched', () => {
		expect(prefixImageTokens('hi', [f('a.txt')], ['/tmp/a.txt'])).toBe('hi');
	});
});
