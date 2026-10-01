import { fmtSize } from './attachments';

export type ImageMarker = { alt: string; id: string; start: number; end: number };

/** The daemon's rewritten marker for an agent-posted image: only this scheme is
 *  an image, and the id is a server-minted uuid. */
const CCTUI_IMG = /!\[([^[\]]*)\]\(cctui-img:\/\/([A-Za-z0-9-]+)\)/g;

export function scanImageMarkers(text: string): ImageMarker[] {
	const out: ImageMarker[] = [];
	for (const m of text.matchAll(CCTUI_IMG)) {
		out.push({
			alt: m[1],
			id: m[2],
			start: m.index ?? 0,
			end: (m.index ?? 0) + m[0].length
		});
	}
	return out;
}

export function imagePlaceholderLabel(
	name: string,
	dimensions: [number, number] | null = null,
	bytes: number | null = null
): string {
	const trimmed = name.trim();
	let out = trimmed ? `image: ${trimmed}` : 'image';
	if (dimensions) out += ` ${dimensions[0]}x${dimensions[1]}`;
	if (bytes !== null) out += ` ${fmtSize(bytes)}`;
	return out;
}

export function inlineImagePlaceholder(
	name: string,
	dimensions: [number, number] | null = null,
	bytes: number | null = null
): string {
	return `[${imagePlaceholderLabel(name, dimensions, bytes)}]`;
}

export function substituteImageMarkers(text: string): string {
	const markers = scanImageMarkers(text);
	if (markers.length === 0) return text;
	let out = '';
	let at = 0;
	for (const marker of markers) {
		out += text.slice(at, marker.start) + inlineImagePlaceholder(marker.alt);
		at = marker.end;
	}
	return out + text.slice(at);
}

export function isOnlyImages(text: string): boolean {
	const markers = scanImageMarkers(text);
	if (markers.length === 0) return false;
	let rest = '';
	let at = 0;
	for (const marker of markers) {
		rest += text.slice(at, marker.start);
		at = marker.end;
	}
	return (rest + text.slice(at)).trim() === '';
}
