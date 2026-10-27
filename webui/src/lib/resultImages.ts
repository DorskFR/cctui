import { fmtSize } from './attachments';
import { escapeHtml } from './highlight';
import { attachmentBlobUrl } from './queries/types';

export type ResultImage =
	| { kind: 'blob'; hash: string; mediaType: string | null; size: number | null }
	| { kind: 'inline'; mediaType: string; data: string }
	| { kind: 'unreadable'; mediaType: string | null; size: number | null };

/** `text` is shown beside the drawn `images`; `labelled` names every image,
 *  for copies that carry no images (export). */
export type ParsedResult = { text: string; images: ResultImage[]; labelled: string };

const HASH = /^[0-9a-f]{64}$/;
const BASE64 = /^[A-Za-z0-9+/]+={0,2}$/;
// Only raster types an <img> can never run script from are drawn inline.
const INLINE_TYPES = new Set(['image/png', 'image/jpeg', 'image/gif', 'image/webp']);

function imageOf(source: unknown): ResultImage | null {
	if (!source || typeof source !== 'object') return null;
	const s = source as Record<string, unknown>;
	const mediaType = typeof s.media_type === 'string' ? s.media_type : null;
	const size = typeof s.size === 'number' ? s.size : null;
	if (s.type === 'cctui-blob' && typeof s.blob_id === 'string' && HASH.test(s.blob_id)) {
		// `encoding: base64`: the stored bytes are base64 text that no longer
		// decodes (secret redaction rewrote part of it).
		if (s.encoding === 'base64') return { kind: 'unreadable', mediaType, size };
		return { kind: 'blob', hash: s.blob_id, mediaType, size };
	}
	if (s.type === 'base64' && typeof s.data === 'string') {
		if (mediaType && INLINE_TYPES.has(mediaType) && BASE64.test(s.data)) {
			return { kind: 'inline', mediaType, data: s.data };
		}
		return { kind: 'unreadable', mediaType, size: null };
	}
	return null;
}

function imageBytes(img: ResultImage): number | null {
	if (img.kind !== 'inline') return img.size;
	const pad = img.data.endsWith('==') ? 2 : img.data.endsWith('=') ? 1 : 0;
	return (img.data.length / 4) * 3 - pad;
}

export function resultImageLabel(img: ResultImage): string {
	const bytes = imageBytes(img);
	const type = img.mediaType ?? 'image';
	const label = bytes === null ? `image ${type}` : `image ${type} ${fmtSize(bytes)}`;
	return img.kind === 'unreadable' ? `[${label}, unreadable]` : `[${label}]`;
}

/** `null` unless `summary` is a stringified array of text and image blocks. */
export function parseResultImages(summary: string): ParsedResult | null {
	const head = summary.trimStart();
	if (!head.startsWith('[') || !head.includes('"image"')) return null;
	let blocks: unknown;
	try {
		blocks = JSON.parse(head);
	} catch {
		return null;
	}
	if (!Array.isArray(blocks)) return null;
	const texts: string[] = [];
	const images: ResultImage[] = [];
	for (const b of blocks) {
		if (!b || typeof b !== 'object') return null;
		const block = b as Record<string, unknown>;
		if (block.type === 'text' && typeof block.text === 'string') {
			texts.push(block.text);
		} else if (block.type === 'image') {
			const img = imageOf(block.source);
			if (!img) return null;
			images.push(img);
		} else {
			return null;
		}
	}
	if (images.length === 0) return null;
	const shown = images.filter((i) => i.kind !== 'unreadable');
	const labels = (texts.length ? images.filter((i) => i.kind === 'unreadable') : images).map(
		resultImageLabel
	);
	return {
		text: [...texts, ...labels].join('\n\n'),
		images: shown,
		labelled: [...texts, ...images.map(resultImageLabel)].join('\n\n')
	};
}

export function resultDisplayText(summary: string): string {
	return parseResultImages(summary)?.labelled ?? summary;
}

export function resultImageSrc(img: ResultImage, sessionId: string): string | null {
	if (img.kind === 'blob') return attachmentBlobUrl(sessionId, img.hash);
	if (img.kind === 'inline') return `data:${img.mediaType};base64,${img.data}`;
	return null;
}

export function renderResultImages(images: ResultImage[], sessionId: string): string {
	return images
		.map((img) => {
			const raw = resultImageSrc(img, sessionId);
			if (!raw) return '';
			const src = escapeHtml(raw);
			const alt = escapeHtml(resultImageLabel(img));
			return (
				`<img class="md-img" src="${src}" alt="${alt}" loading="lazy" ` +
				`data-lightbox="${src}" title="Click to open full image" />`
			);
		})
		.join('');
}
