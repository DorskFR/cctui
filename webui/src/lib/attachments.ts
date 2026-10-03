import type { UploadCaps } from '@bindings/UploadCaps';

// Shared file-attachment helpers for the spawn modal and the mid-chat
// composer: one source of truth for caps, unique-name merging, error
// derivation, and size formatting. The caps are the server's own, served on
// `GET /version`; rejecting here only fails fast, the server is the gate.

export const MAX_FILE_BYTES = 5 * 1024 * 1024;
export const MAX_TOTAL_BYTES = 20 * 1024 * 1024;
export const MAX_FILES = 10;

/** What the server falls back to when the instance has stored no caps. */
export const DEFAULT_UPLOAD_CAPS: UploadCaps = {
	max_files: MAX_FILES,
	max_file_bytes: MAX_FILE_BYTES,
	max_total_bytes: MAX_TOTAL_BYTES
};

/** Split `name` into stem and extension (`a.tar.gz` → `a.tar` + `.gz`). */
function splitExt(name: string): [string, string] {
	const i = name.lastIndexOf('.');
	return i > 0 ? [name.slice(0, i), name.slice(i)] : [name, ''];
}

/** Merge `incoming` into `current` with names kept unique: a clash is renamed
 *  `stem-2.ext`, `stem-3.ext`, … (never replaced), so the list always matches
 *  what the daemon stages. Returns the merged list and the incoming files as
 *  actually added (post-rename), for tokenizing. */
export function mergeFilesRenamed(
	current: File[],
	incoming: File[]
): { files: File[]; added: File[] } {
	const taken = new Set(current.map((f) => f.name));
	const added: File[] = [];
	for (const f of incoming) {
		let file = f;
		if (taken.has(f.name)) {
			const [stem, ext] = splitExt(f.name);
			let n = 2;
			while (taken.has(`${stem}-${n}${ext}`)) n++;
			file = new File([f], `${stem}-${n}${ext}`, { type: f.type, lastModified: f.lastModified });
		}
		taken.add(file.name);
		added.push(file);
	}
	return { files: [...current, ...added], added };
}

/** Merge `incoming` into `current`, renaming duplicate names (see
 *  `mergeFilesRenamed`). */
export function mergeFiles(current: File[], incoming: File[]): File[] {
	return mergeFilesRenamed(current, incoming).files;
}

/** Short inline marker for the attachment at `index`: `[#1]`, `[#2]`, …
 *  The number is the file's position in the list, so it stays derivable from
 *  the persisted list alone. Drafts saved before still carry `[📎N]`. */
export const clipToken = (index: number) => `[#${index + 1}]`;

export const CLIP_TOKEN = /\[(?:#|📎)(\d+)\]/gu;

/** How attaching marks the draft: a short `[#N]`, the full `[name]` (a masked
 *  paste, whose name is already short), or nothing. */
export type FileTokenMode = 'clip' | 'name' | false;

/** Splice `tokens` into `text` at `caret` (end when omitted), space-separated
 *  from the words around it. The caret comes back just after the tokens. */
export function insertTokens(
	text: string,
	tokens: string[],
	caret?: number
): { text: string; caret: number } {
	if (!tokens.length) return { text, caret: caret ?? text.length };
	const at = Math.max(0, Math.min(text.length, caret ?? text.length));
	const before = text.slice(0, at);
	const after = text.slice(at);
	const head = `${before}${before && !/\s$/.test(before) ? ' ' : ''}${tokens.join(' ')}`;
	const gap = after && !/^\s/.test(after) ? ' ' : '';
	return { text: head + gap + after, caret: head.length };
}

/** Add `incoming` to `files`, marking each (post-rename) file in `text` at
 *  `caret` per `mode`. A `[name]` already in the draft is not repeated. */
export function attachFiles(
	files: File[],
	text: string,
	incoming: File[],
	mode: FileTokenMode = 'clip',
	caret?: number
): { files: File[]; text: string; caret: number } {
	const merged = mergeFilesRenamed(files, incoming);
	const base = merged.files.length - merged.added.length;
	const tokens =
		mode === 'clip'
			? merged.added.map((_, i) => clipToken(base + i))
			: mode === 'name'
				? merged.added.map((f) => `[${f.name}]`).filter((t) => !text.includes(t))
				: [];
	const next = insertTokens(text, tokens, caret);
	return { files: merged.files, ...next };
}

/** Follow the list from `before` to `after` (names, in order): each `[#N]`
 *  takes its file's new number, and a removed file's token goes with it. */
export function renumberClipTokens(text: string, before: string[], after: string[]): string {
	return text
		.replace(/ ?\[(?:#|📎)(\d+)\]/gu, (tok, n: string) => {
			const name = before[Number(n) - 1];
			const j = name === undefined ? -1 : after.indexOf(name);
			if (j < 0) return name === undefined ? tok : '';
			return `${tok.startsWith(' ') ? ' ' : ''}${clipToken(j)}`;
		})
		.replace(/^ +/, '');
}

/** Swap each `[#N]` for `[name]`: the staged name from `paths` when the
 *  upload returned one, else the file's own. A number past the list points at
 *  nothing the agent will receive, so it is dropped. */
export function expandClipTokens(text: string, files: File[], paths: string[] = []): string {
	return text
		.replace(/ ?\[(?:#|📎)(\d+)\]/gu, (tok, n: string) => {
			const i = Number(n) - 1;
			const file = files[i];
			if (!file) return '';
			return `${tok.startsWith(' ') ? ' ' : ''}[${paths[i]?.split('/').pop() || file.name}]`;
		})
		.replace(/^ +/, '');
}

/** `#1 a.png · #2 b.pdf`: what each inline marker points at. */
export function clipLegend(files: File[]): string {
	return files.map((f, i) => `#${i + 1} ${f.name}`).join(' · ');
}

const PASTE_NAME = /\bpaste-(\d+)\.txt\b/g;

/** Next free `paste-N.txt` index: max N over attachment names, `[paste-N.txt]`
 *  tokens in `text` and `used` (the names the session already staged), plus one.
 *  Derived, not counted, so it survives a remount whose draft still references
 *  earlier pastes. Without `used` every fresh draft would restart at
 *  `paste-1.txt` and collide with an earlier message's upload. */
export function nextPasteIndex(files: File[], text: string, used: Iterable<string> = []): number {
	let max = 0;
	const scan = (s: string) => {
		for (const m of s.matchAll(PASTE_NAME)) max = Math.max(max, Number(m[1]));
	};
	for (const f of files) scan(f.name);
	for (const name of used) scan(name);
	scan(text);
	return max + 1;
}

/** Pasted text at least this long collapses into a `paste-N.txt` attachment
 *  (the Claude Code trick) instead of flooding the textarea. */
export const PASTE_MASK_CHARS = 2000;

/** The `paste-N.txt` file a long text paste collapses into, or null when the
 *  paste is short enough to land in the field. */
export function maskedPaste(
	text: string,
	files: File[],
	draft: string,
	used: Iterable<string> = []
): File | null {
	if (!text || text.length < PASTE_MASK_CHARS) return null;
	const name = `paste-${nextPasteIndex(files, draft, used)}.txt`;
	return new File([text], name, { type: 'text/plain' });
}

/** Point each `[name]` token at the name staging actually gave the file.
 *  `paths` is the staged absolute path per entry of `files`, in order; a clash
 *  is renamed server-side (`paste-1.txt` → `paste-1-1.txt`) and a token left
 *  on the old name resolves to some other message's upload. */
export function rewriteFileTokens(text: string, files: File[], paths: string[]): string {
	let out = text;
	files.forEach((f, i) => {
		const staged = paths[i]?.split('/').pop();
		if (!staged || staged === f.name) return;
		out = out.split(`[${f.name}]`).join(`[${staged}]`);
	});
	return out;
}

const LEADING_TOKEN_RUN_RE = /^(?:\s*\[[^[\]\n]+\])+/;

/** Claude swaps a staged image path for an image block and records only an
 *  `[Image #N]` in its place, so a leading `[name]` run is the one copy of the
 *  image's name its transcript keeps. */
export function prefixImageTokens(text: string, files: File[], paths: string[]): string {
	const run = LEADING_TOKEN_RUN_RE.exec(text)?.[0] ?? '';
	const tokens = files
		.map((f, i) => (f.type.startsWith('image/') ? `[${paths[i]?.split('/').pop() || f.name}]` : ''))
		.filter((t) => t && !run.includes(t));
	if (!tokens.length) return text;
	return text ? `${tokens.join(' ')}\n${text}` : tokens.join(' ');
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
export function fileCapError(files: File[], caps: UploadCaps = DEFAULT_UPLOAD_CAPS): string {
	const total = files.reduce((n, f) => n + f.size, 0);
	if (files.some((f) => f.size > caps.max_file_bytes))
		return `A file exceeds the ${fmtSize(caps.max_file_bytes)} per-file cap`;
	if (files.length > caps.max_files) return `Too many files (max ${caps.max_files})`;
	if (total > caps.max_total_bytes)
		return `Attachments exceed the ${fmtSize(caps.max_total_bytes)} total cap`;
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
