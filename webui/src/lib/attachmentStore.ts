import { del, get, keys, set } from 'idb-keyval';
import { MAX_TOTAL_BYTES } from './attachments';

// Draft text lives in localStorage (drafts.ts); File handles cannot, so they
// go to IndexedDB under a key derived from the draft key. Without IndexedDB
// (SSR, tests) an in-memory map still covers a remount within the page.
const PREFIX = 'cctui_files_';
export const attachmentKey = (draftKey: string) => `${PREFIX}${draftKey}`;

// Names are always recorded; files only while under the cap, so a restore can
// tell "never attached" from "attached but lost".
interface Record {
	names: string[];
	files: File[];
}

export interface RestoredAttachments {
	files: File[];
	missing: string[];
}

const hasIdb = () => typeof indexedDB !== 'undefined';
const memory = new Map<string, Record>();

async function read(key: string): Promise<Record | undefined> {
	if (!hasIdb()) return memory.get(key);
	try {
		return await get<Record>(key);
	} catch {
		return undefined;
	}
}

async function write(key: string, rec: Record): Promise<void> {
	if (!hasIdb()) {
		memory.set(key, rec);
		return;
	}
	try {
		await set(key, rec);
	} catch {
		if (rec.files.length) await write(key, { names: rec.names, files: [] });
	}
}

async function remove(key: string): Promise<void> {
	if (!hasIdb()) {
		memory.delete(key);
		return;
	}
	try {
		await del(key);
	} catch {
		// best effort
	}
}

export const attachmentStore = {
	async get(draftKey: string): Promise<RestoredAttachments> {
		const rec = await read(attachmentKey(draftKey));
		if (!rec) return { files: [], missing: [] };
		const files = Array.isArray(rec.files) ? rec.files.filter((f) => f instanceof Blob) : [];
		const present = new Set(files.map((f) => f.name));
		const names = Array.isArray(rec.names) ? rec.names : [];
		return { files, missing: names.filter((n) => !present.has(n)) };
	},
	/** Empty list removes the record. Over-cap lists record names only. */
	async set(draftKey: string, files: File[]): Promise<void> {
		const key = attachmentKey(draftKey);
		if (files.length === 0) return remove(key);
		const total = files.reduce((n, f) => n + f.size, 0);
		const names = files.map((f) => f.name);
		await write(key, { names, files: total > MAX_TOTAL_BYTES ? [] : files });
	},
	clear(draftKey: string): Promise<void> {
		return remove(attachmentKey(draftKey));
	},
	async clearAll(): Promise<void> {
		if (!hasIdb()) {
			memory.clear();
			return;
		}
		try {
			const all = await keys();
			await Promise.all(
				all.filter((k) => typeof k === 'string' && k.startsWith(PREFIX)).map((k) => del(k))
			);
		} catch {
			// best effort
		}
	}
};

const TOKEN = /\[([^[\]\n]+)\]/g;

/** Remove `[name]` tokens whose name is in `missing`, collapsing the
 *  surrounding whitespace. */
export function dropMissingTokens(
	text: string,
	missing: string[]
): { text: string; dropped: number } {
	if (missing.length === 0) return { text, dropped: 0 };
	const gone = new Set(missing);
	let dropped = 0;
	const out = text
		.replace(TOKEN, (tok, name: string) => {
			if (!gone.has(name)) return tok;
			dropped++;
			return '';
		})
		.replace(/[ \t]{2,}/g, ' ')
		.replace(/[ \t]+$/gm, '');
	return { text: dropped ? out : text, dropped };
}
