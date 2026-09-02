// Shared file-attachment helpers for the spawn modal and the mid-chat
// composer: one source of truth for caps, dedupe-by-name merging, error
// derivation, and size formatting. Mirrors the server caps in
// `cctui-server/src/uploads.rs` so we reject before uploading.

export const MAX_FILE_BYTES = 5 * 1024 * 1024;
export const MAX_TOTAL_BYTES = 20 * 1024 * 1024;
export const MAX_FILES = 10;

/** Merge `incoming` into `current`, de-duping by filename (a re-pick / re-drop
 *  of the same name replaces rather than double-adds). */
export function mergeFiles(current: File[], incoming: File[]): File[] {
	const byName = new Map(current.map((f) => [f.name, f]));
	for (const f of incoming) byName.set(f.name, f);
	return [...byName.values()];
}

/** Append a `[name]` reference for each attached file to the draft text,
 *  skipping names it already contains so a re-pick doesn't duplicate. */
export function appendFileTokens(text: string, files: File[]): string {
	let out = text;
	for (const f of files) {
		const token = `[${f.name}]`;
		if (out.includes(token)) continue;
		out = out && !/\s$/.test(out) ? `${out} ${token}` : `${out}${token}`;
	}
	return out;
}

/** Drop the file with `name` from the list. */
export function removeFileByName(files: File[], name: string): File[] {
	return files.filter((f) => f.name !== name);
}

/** Validate a file list against the caps; returns a human error or '' if ok. */
export function fileCapError(files: File[]): string {
	const total = files.reduce((n, f) => n + f.size, 0);
	if (files.some((f) => f.size > MAX_FILE_BYTES))
		return `A file exceeds the ${MAX_FILE_BYTES / 1024 / 1024} MB per-file cap`;
	if (files.length > MAX_FILES) return `Too many files (max ${MAX_FILES})`;
	if (total > MAX_TOTAL_BYTES)
		return `Attachments exceed the ${MAX_TOTAL_BYTES / 1024 / 1024} MB total cap`;
	return '';
}

/** Human-readable byte size. */
export function fmtSize(n: number): string {
	if (n < 1024) return `${n} B`;
	if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
	return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

// ── Clipboard → files ────────────────────────────────────────────
// Common clipboard MIME types → file extensions. A pasted screenshot has no
// filename, so we synthesize `clipboard-N.<ext>`; anything unmapped falls back
// to the MIME subtype (sanitised) or `.bin`.
const MIME_EXT: Record<string, string> = {
	'image/png': 'png',
	'image/jpeg': 'jpg',
	'image/gif': 'gif',
	'image/webp': 'webp',
	'image/bmp': 'bmp',
	'image/svg+xml': 'svg',
	'image/tiff': 'tiff',
	'application/pdf': 'pdf'
};
export function extForType(type: string): string {
	if (MIME_EXT[type]) return MIME_EXT[type];
	const sub = type.split('/')[1]?.split(';')[0];
	return sub ? sub.replace(/[^a-z0-9]/gi, '') || 'bin' : 'bin';
}

/** Stateful clipboard-file extractor: one per composer/form so the synthesized
 *  `clipboard-N.<ext>` names stay unique within that surface. */
export function makeClipboardFiles() {
	let counter = 1;
	// Give a clipboard blob a stable, unique filename if it has none (pasted
	// screenshots/images arrive nameless), so dedupe-by-name doesn't collapse them.
	const named = (f: File): File => {
		if (f.name?.trim()) return f;
		const ext = extForType(f.type || 'application/octet-stream');
		return new File([f], `clipboard-${counter++}.${ext}`, {
			type: f.type || 'application/octet-stream'
		});
	};
	// Extract binary files from a paste (copied files OR a pasted image/screenshot).
	// Prefer `items` (some browsers expose pasted screenshots only there, not in
	// `.files`), then fall back to `.files`.
	return (cd: DataTransfer): File[] => {
		const out: File[] = [];
		for (const item of Array.from(cd.items ?? [])) {
			if (item.kind !== 'file') continue;
			const f = item.getAsFile();
			if (f) out.push(named(f));
		}
		if (out.length === 0) {
			for (const f of Array.from(cd.files ?? [])) out.push(named(f));
		}
		return out;
	};
}
