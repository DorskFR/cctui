import { describe, expect, it } from 'vitest';
import {
	DEFAULT_UPLOAD_CAPS,
	MAX_FILES,
	MAX_FILE_BYTES,
	MAX_TOTAL_BYTES,
	appendFileTokens,
	extForType,
	fileCapError,
	fmtSize,
	mergeFilesRenamed,
	nextPasteIndex,
	rewriteFileTokens
} from '$lib/attachments';
import { parityFixture } from './fixtures';

type Fixture = {
	MAX_FILE_BYTES: number;
	MAX_TOTAL_BYTES: number;
	MAX_FILES: number;
	fmtSize: { bytes: number; text: string }[];
	capError: { perFile: string; tooMany: string; total: string };
	uniqueName: { taken: string[]; incoming: string; name: string }[];
	nextPasteIndex: { names: string[]; text: string; used: string[]; next: number }[];
	appendFileTokens: { text: string; names: string[]; out: string }[];
	rewriteFileTokens: { text: string; names: string[]; paths: string[]; out: string }[];
	extForType: { type: string; ext: string }[];
};

const fx = parityFixture<Fixture>('attachments');

/** A File of `size` bytes — only name and size matter to these helpers. */
const file = (name: string, size = 1, type = ''): File =>
	new File([new Uint8Array(size)], name, { type });

describe('attachment parity fixtures', () => {
	it('mirrors the caps cctui-proto compiles in', () => {
		expect(MAX_FILE_BYTES).toBe(fx.MAX_FILE_BYTES);
		expect(MAX_TOTAL_BYTES).toBe(fx.MAX_TOTAL_BYTES);
		expect(MAX_FILES).toBe(fx.MAX_FILES);
		expect(DEFAULT_UPLOAD_CAPS).toEqual({
			max_files: fx.MAX_FILES,
			max_file_bytes: fx.MAX_FILE_BYTES,
			max_total_bytes: fx.MAX_TOTAL_BYTES
		});
	});

	it('words byte sizes the way cctui-clientcore does', () => {
		for (const c of fx.fmtSize) expect(fmtSize(c.bytes), `${c.bytes}`).toBe(c.text);
	});

	it('words each cap breach the way cctui-clientcore does', () => {
		expect(fileCapError([file('a', 1)])).toBe('');
		expect(fileCapError([file('big', fx.MAX_FILE_BYTES + 1)])).toBe(fx.capError.perFile);
		const many = Array.from({ length: fx.MAX_FILES + 1 }, (_, i) => file(`f${i}`, 1));
		expect(fileCapError(many)).toBe(fx.capError.tooMany);
		const big = Array.from({ length: 5 }, (_, i) => file(`f${i}`, fx.MAX_FILE_BYTES));
		expect(fileCapError(big)).toBe(fx.capError.total);
	});

	it('renames a clashing attachment the way cctui-clientcore does', () => {
		for (const c of fx.uniqueName) {
			const { added } = mergeFilesRenamed(c.taken.map((n) => file(n)), [file(c.incoming)]);
			expect(added[0].name, `${c.taken} + ${c.incoming}`).toBe(c.name);
		}
	});

	it('derives the next paste index the way cctui-clientcore does', () => {
		for (const c of fx.nextPasteIndex) {
			const got = nextPasteIndex(c.names.map((n) => file(n)), c.text, c.used);
			expect(got, `${JSON.stringify(c)}`).toBe(c.next);
		}
	});

	it('appends and rewrites file tokens the way cctui-clientcore does', () => {
		for (const c of fx.appendFileTokens)
			expect(appendFileTokens(c.text, c.names.map((n) => file(n)))).toBe(c.out);
		for (const c of fx.rewriteFileTokens)
			expect(rewriteFileTokens(c.text, c.names.map((n) => file(n)), c.paths)).toBe(c.out);
	});

	it('maps clipboard MIME types the way cctui-clientcore does', () => {
		for (const c of fx.extForType) expect(extForType(c.type), c.type).toBe(c.ext);
	});
});
