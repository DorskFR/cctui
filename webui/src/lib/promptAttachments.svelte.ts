// Construct during component init: it opens effects.
import { tick } from 'svelte';
import { errMessage } from '$lib/api';
import {
	attachFiles,
	fileCapError,
	makeClipboardFiles,
	maskedPaste,
	prefixImageTokens,
	removeFileByName,
	removeFileToken,
	rewriteFileTokens,
	type FileTokenMode
} from '$lib/attachments';
import { attachmentDraftSync, restoreDraftTokens } from '$lib/attachmentStore';
import { uploadCaps } from '$lib/uploadCaps.svelte';
import { imageAttachments } from '$lib/imageAttachments.svelte';
import { toasts } from '$lib/toast.svelte';
import { m } from '$lib/paraglide/messages';

export interface PromptAttachmentsOpts {
	input: () => string;
	setInput: (text: string) => void;
	/** The prompt's textarea: attached files are marked at its caret. */
	el?: () => HTMLTextAreaElement | null | undefined;
	enabled?: () => boolean;
	/** Names already uploaded, so the next masked paste skips them. */
	stagedNames?: () => string[];
	/** Persist the list under this key, restoring it whenever the key changes.
	 *  Without it the owner calls `restore` and stores the list itself. */
	draftKey?: () => string;
	sync?: ReturnType<typeof attachmentDraftSync>;
}

export class PromptAttachments {
	#o: PromptAttachmentsOpts;
	#sync: ReturnType<typeof attachmentDraftSync>;
	#clipboardFiles = makeClipboardFiles();
	files = $state<File[]>([]);
	uploading = $state(false);
	dragActive = $state(false);
	images = imageAttachments();
	// Key of the loaded draft; null while a restore is in flight so a key
	// switch never writes the old list under the new key.
	#key = $state<string | null>(null);
	error = $derived(fileCapError(this.files, uploadCaps));

	constructor(o: PromptAttachmentsOpts) {
		this.#o = o;
		this.#sync = o.sync ?? attachmentDraftSync();
		$effect(() => () => this.images.reset());
		const draftKey = o.draftKey;
		if (!draftKey) return;
		$effect(() => {
			if (!this.#key) return;
			void this.#sync.persist(this.#key, [...this.files]);
		});
		$effect(() => {
			const key = draftKey();
			this.images.reset();
			this.#key = null;
			this.files = [];
			let live = true;
			void this.restore(key).then((ok) => {
				if (live && ok) this.#key = key;
			});
			return () => {
				live = false;
			};
		});
	}

	/** Load the list stored under `key`, keeping anything attached while the
	 *  read was in flight, and drop the draft's markers of files that did not
	 *  survive. False when a later restore or a send superseded this one. */
	async restore(key: string, extra?: (files: File[]) => File[]): Promise<boolean> {
		const restored = await this.#sync.restore(key);
		if (!restored) return false;
		const restoredNames = new Set(restored.files.map((f) => f.name));
		const merged = [...restored.files, ...this.files.filter((f) => !restoredNames.has(f.name))];
		this.files = extra ? extra(merged) : merged;
		const present = new Set(this.files.map((f) => f.name));
		const missing = restored.missing.filter((n) => !present.has(n));
		const { text, dropped } = restoreDraftTokens(this.#o.input(), { ...restored, missing });
		if (dropped) {
			this.#o.setInput(text);
			toasts.info(m.attachments_missing_dropped({ count: dropped }));
		}
		return true;
	}

	#enabled(): boolean {
		return (this.#o.enabled?.() ?? true) && !this.uploading;
	}

	/** Stage `incoming`, marking each file's `[name]` at the caret. */
	add = (incoming: File[], mode: FileTokenMode = 'name'): void => {
		if (!this.#enabled()) return;
		const el = this.#o.el?.();
		let caret = el ? el.selectionStart : undefined;
		this.images.add(
			incoming,
			(file) => {
				const next = attachFiles(this.files, this.#o.input(), [file], mode, caret);
				caret = next.caret;
				this.files = next.files;
				this.#o.setInput(next.text);
				if (el && document.activeElement === el) {
					const at = next.caret;
					void tick().then(() => el.setSelectionRange(at, at));
				}
			},
			(file) => toasts.error(m.attachments_compression_failed({ name: file.name }))
		);
	};

	remove = (name: string): void => {
		this.files = removeFileByName(this.files, name);
		const text = this.#o.input();
		const next = removeFileToken(text, name);
		if (next !== text) this.#o.setInput(next);
	};

	setDragActive = (active: boolean): void => {
		this.dragActive = active;
	};

	/** Binary clipboard content attaches like the picker/drop; a large text
	 *  paste becomes a `paste-N.txt`; anything else pastes normally. */
	onPaste = (e: ClipboardEvent): void => {
		if (!this.#enabled()) return;
		const cd = e.clipboardData;
		if (!cd) return;
		const files = this.#clipboardFiles(cd);
		if (files.length > 0) {
			e.preventDefault();
			this.add(files);
			return;
		}
		const text = cd.getData('text/plain');
		const paste = maskedPaste(text, this.files, this.#o.input(), this.#o.stagedNames?.() ?? []);
		if (!paste) return;
		e.preventDefault();
		this.add([paste]);
		toasts.ok(m.composer_large_paste({ name: paste.name, lines: text.split('\n').length }));
	};

	/** Upload the staged files and fold their paths under `text`. Returns the
	 *  final body, or null when the upload failed (draft + files kept intact). */
	async stage(
		text: string,
		stageFiles: (files: File[]) => Promise<{ paths: string[] }>
	): Promise<string | null> {
		if (!this.files.length) return text;
		this.uploading = true;
		try {
			const { paths } = await stageFiles(this.files);
			const prose = prefixImageTokens(rewriteFileTokens(text, this.files, paths), this.files, paths);
			const list = paths.map((p) => `- ${p}`).join('\n');
			const header = paths.length === 1 ? 'Attached file:' : `Attached files (${paths.length}):`;
			this.files = [];
			const key = this.#o.draftKey?.();
			if (key) {
				this.#key = key;
				void this.#sync.discard(key);
			}
			return prose ? `${prose}\n\n${header}\n${list}` : `${header}\n${list}`;
		} catch (e) {
			toasts.error(m.composer_attachment_upload_failed({ message: errMessage(e) }));
			return null;
		} finally {
			this.uploading = false;
		}
	}
}
