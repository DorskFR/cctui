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
