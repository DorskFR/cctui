import { describe, expect, it } from 'vitest';
import { DEFAULT_UPLOAD_CAPS, MAX_FILES, MAX_FILE_BYTES, MAX_TOTAL_BYTES } from '$lib/attachments';
import { parityFixture } from './fixtures';

type Fixture = { MAX_FILE_BYTES: number; MAX_TOTAL_BYTES: number; MAX_FILES: number };

const fx = parityFixture<Fixture>('attachments');

describe('attachment caps parity fixtures', () => {
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
});
