// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { compressImage, needsImageCompression } from './compressImage';
import { DEFAULT_UPLOAD_CAPS, MAX_FILE_BYTES } from './attachments';
import { setUploadCaps } from './uploadCaps.svelte';

const large = (name = 'screen.png', type = 'image/png') => new File([new Uint8Array(MAX_FILE_BYTES + 1)], name, { type });
const sized = (bytes: number, name = 'screen.png') => new File([new Uint8Array(bytes)], name, { type: 'image/png' });
afterEach(() => {
	vi.restoreAllMocks();
	setUploadCaps(DEFAULT_UPLOAD_CAPS);
});

describe('compressImage', () => {
	it('preserves small images and oversized non-images', async () => {
		for (const file of [new File(['small'], 'a.png', { type: 'image/png' }), large('a.pdf', 'application/pdf')]) {
			expect(await compressImage(file)).toBe(file);
		}
		expect(needsImageCompression(large('photo.JPG', ''))).toBe(true);
		expect(needsImageCompression(new File([new Uint8Array(MAX_FILE_BYTES)], 'a.png', { type: 'image/png' }))).toBe(false);
	});

	function mockCanvas(sizes: number[], broken = false) {
		vi.spyOn(URL, 'createObjectURL').mockReturnValue('blob:test');
		const revoke = vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => {});
		vi.spyOn(Image.prototype, 'decode').mockImplementation(async () => {
			if (broken) throw new Error('Unsupported image');
		});
		vi.spyOn(Image.prototype, 'naturalWidth', 'get').mockReturnValue(6000);
		vi.spyOn(Image.prototype, 'naturalHeight', 'get').mockReturnValue(3000);
		const draw = vi.fn();
		vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue({ fillRect: vi.fn(), drawImage: draw } as unknown as CanvasRenderingContext2D);
		const encode = vi.spyOn(HTMLCanvasElement.prototype, 'toBlob').mockImplementation((cb) => {
			cb(new Blob([new Uint8Array(sizes.shift() ?? 100)], { type: 'image/jpeg' }));
		});
		return { revoke, draw, encode };
	}

	it('tries quality first, then reduces dimensions and cleans up', async () => {
		const { revoke, draw, encode } = mockCanvas([MAX_FILE_BYTES + 1, MAX_FILE_BYTES + 1, MAX_FILE_BYTES + 1, 100]);
		const result = await compressImage(large());
		expect(result.name).toBe('screen.jpg');
		expect(result.type).toBe('image/jpeg');
		expect(result.size).toBe(100);
		expect(encode.mock.calls.map((call) => call[2])).toEqual([0.92, 0.85, 0.75, 0.92]);
		expect(draw.mock.calls.map((call) => call.slice(3))).toEqual([[4096, 2048], [3072, 1536]]);
		expect(revoke).toHaveBeenCalledWith('blob:test');
	});

	it('releases resources on decode failure', async () => {
		const { revoke } = mockCanvas([], true);
		await expect(compressImage(large())).rejects.toThrow('Unsupported image');
		expect(revoke).toHaveBeenCalled();
	});

	describe('configured per-file cap', () => {
		it('leaves an image under the served cap untouched', async () => {
			const { encode } = mockCanvas([100]);
			setUploadCaps({ ...DEFAULT_UPLOAD_CAPS, max_file_bytes: MAX_FILE_BYTES * 4 });
			const file = sized(MAX_FILE_BYTES * 2);
			expect(needsImageCompression(file)).toBe(false);
			expect(await compressImage(file)).toBe(file);
			expect(encode).not.toHaveBeenCalled();
		});

		it('compresses down to a lowered served cap', async () => {
			const cap = 1024;
			setUploadCaps({ ...DEFAULT_UPLOAD_CAPS, max_file_bytes: cap });
			const { encode } = mockCanvas([cap + 1, cap]);
			const file = sized(cap * 2);
			expect(needsImageCompression(file)).toBe(true);
			expect((await compressImage(file)).size).toBe(cap);
			expect(encode.mock.calls.map((call) => call[2])).toEqual([0.92, 0.85]);
		});

		it('honours an explicitly injected cap over the served one', async () => {
			setUploadCaps({ ...DEFAULT_UPLOAD_CAPS, max_file_bytes: 1 });
			const file = sized(2048);
			expect(needsImageCompression(file, 4096)).toBe(false);
			expect(await compressImage(file, 4096)).toBe(file);
		});
	});
});
