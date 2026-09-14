import { describe, expect, it, vi } from 'vitest';
import { imageAttachments } from './imageAttachments.svelte';
import { compressImage } from './compressImage';
vi.mock('./compressImage', () => ({
	needsImageCompression: (file: File) => file.type === 'image/png',
	compressImage: vi.fn()
}));
const image = () => new File(['image'], 'photo.png', { type: 'image/png' });

describe('image attachment queue', () => {
	it('serialises concurrent additions and clears the pending indicator', async () => {
		const queue = imageAttachments();
		let finish!: (file: File) => void;
		vi.mocked(compressImage).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; })).mockResolvedValueOnce(image());
		const accept = vi.fn();
		queue.add([image()], accept, vi.fn());
		queue.add([image()], accept, vi.fn());
		expect(queue.pending).toHaveLength(2);
		await vi.waitFor(() => expect(finish).toBeDefined());
		expect(accept).not.toHaveBeenCalled();
		finish(image());
		await vi.waitFor(() => expect(queue.pending).toHaveLength(0));
		expect(accept).toHaveBeenCalledTimes(2);
	});
	it('does not attach a result after the composer is reset', async () => {
		const queue = imageAttachments();
		let finish!: (file: File) => void;
		vi.mocked(compressImage).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
		const accept = vi.fn();
		queue.add([image()], accept, vi.fn());
		await vi.waitFor(() => expect(finish).toBeDefined());
		queue.reset();
		finish(image());
		await new Promise(resolve => setTimeout(resolve, 0));
		expect(queue.pending).toHaveLength(0);
		expect(accept).not.toHaveBeenCalled();
	});
	it('reports a failed image and continues with the next', async () => {
		const queue = imageAttachments();
		vi.mocked(compressImage).mockRejectedValueOnce(new Error('decode')).mockResolvedValueOnce(image());
		const accept = vi.fn(), fail = vi.fn();
		queue.add([image(), image()], accept, fail);
		await vi.waitFor(() => expect(queue.pending).toHaveLength(0));
		expect(accept).toHaveBeenCalledTimes(1);
		expect(fail).toHaveBeenCalledTimes(1);
	});
});
