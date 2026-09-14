import { compressImage, needsImageCompression } from './compressImage';

/** Serialise decoding to avoid holding several large canvases on mobile. */
export function imageAttachments() {
	let pending = $state<{ id: number; file: File }[]>([]);
	let nextId = 0;
	let queue = Promise.resolve();
	let generation = 0;
	return {
		get pending() { return pending; },
		reset() { generation++; pending = []; },
		add(incoming: File[], accept: (file: File) => void, fail: (file: File) => void) {
			const epoch = generation;
			for (const file of incoming) {
				if (!needsImageCompression(file)) { accept(file); continue; }
				const entry = { id: nextId++, file };
				pending = [...pending, entry];
				queue = queue.then(async () => {
					try {
						if (epoch !== generation) return;
						const result = await compressImage(file);
						if (epoch === generation) accept(result);
					} catch {
						if (epoch === generation) fail(file);
					} finally {
						pending = pending.filter((item) => item.id !== entry.id);
					}
				});
			}
		}
	};
}
