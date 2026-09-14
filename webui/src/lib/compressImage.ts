import { MAX_FILE_BYTES } from './attachments';

export function needsImageCompression(file: File): boolean {
	return file.size > MAX_FILE_BYTES && (file.type.startsWith('image/') ||
		(!file.type && /\.(png|jpe?g|webp|gif|bmp|avif|heic|heif|tiff?)$/i.test(file.name)));
}

/** Browser-only conversion. Keep detail before trading resolution for size. */
export async function compressImage(file: File): Promise<File> {
	if (!needsImageCompression(file)) return file;
	const url = URL.createObjectURL(file);
	const canvas = document.createElement('canvas');
	try {
		const image = new Image();
		image.src = url;
		await image.decode();
		if (!image.naturalWidth || !image.naturalHeight) throw new Error('Invalid image dimensions');
		const ctx = canvas.getContext('2d');
		if (!ctx) throw new Error('Canvas unavailable');
		// Bound canvas memory on phones while keeping screenshots legible.
		let scale = Math.min(1, 4096 / Math.max(image.naturalWidth, image.naturalHeight));
		for (let attempt = 0; attempt < 8; attempt++) {
			canvas.width = Math.max(1, Math.round(image.naturalWidth * scale));
			canvas.height = Math.max(1, Math.round(image.naturalHeight * scale));
			ctx.fillStyle = '#ffffff';
			ctx.fillRect(0, 0, canvas.width, canvas.height);
			ctx.drawImage(image, 0, 0, canvas.width, canvas.height);
			for (const quality of [0.92, 0.85, 0.75]) {
				const blob = await new Promise<Blob>((resolve, reject) => {
					canvas.toBlob((result) => result ? resolve(result) : reject(new Error('JPEG encoding failed')), 'image/jpeg', quality);
				});
				if (blob.type !== 'image/jpeg') throw new Error('JPEG encoding unavailable');
				if (blob.size <= MAX_FILE_BYTES) {
					return new File([blob], `${file.name.replace(/\.[^.]+$/, '') || 'image'}.jpg`, {
						type: 'image/jpeg', lastModified: file.lastModified
					});
				}
			}
			scale *= 0.75;
		}
		throw new Error('Image still exceeds attachment limit');
	} finally {
		URL.revokeObjectURL(url);
		canvas.width = canvas.height = 0;
	}
}
