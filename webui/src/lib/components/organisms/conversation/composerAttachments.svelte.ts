// Mid-chat file attachments for the composer. Persisted per session in
// IndexedDB next to the localStorage draft; on send the files are uploaded
// first and the staged paths appended under the message text so the agent
// reads them.
import { errMessage } from '$lib/api';
import {
	attachFiles,
	fileCapError,
	makeClipboardFiles,
	nextPasteIndex,
	removeFileByName,
	rewriteFileTokens
} from '$lib/attachments';
import { attachmentDraftSync, dropMissingTokens } from '$lib/attachmentStore';
import { imageAttachments } from '$lib/imageAttachments.svelte';
import { toasts } from '$lib/toast.svelte';
import { m } from '$lib/paraglide/messages';

// Pasted text at least this long collapses into a `paste-N.txt` attachment
// (the Claude Code trick) instead of flooding the textarea.
const PASTE_MASK_CHARS = 2000;

export interface ComposerAttachmentsOpts {
	draftKey: () => string;
	enabled: () => boolean;
	input: () => string;
	setInput: (text: string) => void;
	/** Names the session has already staged: a new draft has no tokens of its
	 *  own, so these alone keep the next paste off `paste-1.txt`. */
	stagedNames: () => string[];
	sync?: ReturnType<typeof attachmentDraftSync>;
}

export class ComposerAttachments {
	#o: ComposerAttachmentsOpts;
	#sync: ReturnType<typeof attachmentDraftSync>;
	#clipboardFiles = makeClipboardFiles();
	files = $state<File[]>([]);
	uploading = $state(false);
	dragActive = $state(false);
	images = imageAttachments();
	// Key of the session whose attachments are loaded; null while a restore is
	// in flight so a session switch never writes the old list under the new key.
	#key = $state<string | null>(null);
	error = $derived(fileCapError(this.files));

	constructor(o: ComposerAttachmentsOpts) {
		this.#o = o;
		this.#sync = o.sync ?? attachmentDraftSync();
		$effect(() => {
			if (!this.#key) return;
			void this.#sync.persist(this.#key, [...this.files]);
		});
		$effect(() => {
			const key = o.draftKey();
			this.images.reset();
			this.#key = null;
			let live = true;
			(async () => {
				const restored = await this.#sync.restore(key);
				if (!live || !restored) return;
				this.files = restored.files;
				const { text, dropped } = dropMissingTokens(o.input(), restored.missing);
				if (dropped) {
					o.setInput(text);
					toasts.info(m.attachments_missing_dropped({ count: dropped }));
				}
				this.#key = key;
			})();
			return () => {
				live = false;
			};
		});
	}

	add(incoming: File[]): void {
		if (!this.#o.enabled() || this.uploading) return;
		this.images.add(
			incoming,
			(file) => {
				const next = attachFiles(this.files, this.#o.input(), [file]);
				this.files = next.files;
				this.#o.setInput(next.text);
			},
			(file) => toasts.error(m.attachments_compression_failed({ name: file.name }))
		);
	}

	remove(name: string): void {
		this.files = removeFileByName(this.files, name);
	}

	/** Binary clipboard content attaches like the picker/drop; a large text
	 *  paste becomes a `paste-N.txt`; anything else pastes normally. */
	onPaste(e: ClipboardEvent): void {
		if (!this.#o.enabled()) return;
		const cd = e.clipboardData;
		if (!cd) return;
		const files = this.#clipboardFiles(cd);
		if (files.length > 0) {
			e.preventDefault();
			this.add(files);
			return;
		}
		const text = cd.getData('text/plain');
		if (!text || text.length < PASTE_MASK_CHARS) return;
		e.preventDefault();
		const name = `paste-${nextPasteIndex(this.files, this.#o.input(), this.#o.stagedNames())}.txt`;
		this.add([new File([text], name, { type: 'text/plain' })]);
		const lines = text.split('\n').length;
		toasts.ok(m.composer_large_paste({ name, lines }));
	}

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
			const prose = rewriteFileTokens(text, this.files, paths);
			const list = paths.map((p) => `- ${p}`).join('\n');
			const header = paths.length === 1 ? 'Attached file:' : `Attached files (${paths.length}):`;
			this.files = [];
			const key = this.#o.draftKey();
			this.#key = key;
			void this.#sync.discard(key);
			return prose ? `${prose}\n\n${header}\n${list}` : `${header}\n${list}`;
		} catch (e) {
			toasts.error(m.composer_attachment_upload_failed({ message: errMessage(e) }));
			return null;
		} finally {
			this.uploading = false;
		}
	}
}
