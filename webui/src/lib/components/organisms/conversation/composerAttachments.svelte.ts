// Mid-chat file attachments for the composer. Persisted per session in
// IndexedDB next to the localStorage draft; on send the files are uploaded
// first and the staged paths appended under the message text so the agent
// reads them.
import { tick } from 'svelte';
import { errMessage } from '$lib/api';
import {
	attachFiles,
	clipLegend,
	expandClipTokens,
	fileCapError,
	makeClipboardFiles,
	maskedPaste,
	prefixImageTokens,
	removeFileByName,
	renumberClipTokens,
	rewriteFileTokens,
	type FileTokenMode
} from '$lib/attachments';
import { attachmentDraftSync, restoreDraftTokens } from '$lib/attachmentStore';
import { uploadCaps } from '$lib/uploadCaps.svelte';
import { imageAttachments } from '$lib/imageAttachments.svelte';
import { toasts } from '$lib/toast.svelte';
import { m } from '$lib/paraglide/messages';

export interface ComposerAttachmentsOpts {
	draftKey: () => string;
	enabled: () => boolean;
	input: () => string;
	setInput: (text: string) => void;
	/** Names the session has already staged: a new draft has no tokens of its
	 *  own, so these alone keep the next paste off `paste-1.txt`. */
	stagedNames: () => string[];
	/** The draft's textarea: attached files are marked at its caret. */
	el?: () => HTMLTextAreaElement | null | undefined;
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
	error = $derived(fileCapError(this.files, uploadCaps));
	legend = $derived(clipLegend(this.files));

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
				const { text, dropped } = restoreDraftTokens(o.input(), restored);
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

	/** Stage `incoming`, marking each file at the caret: a short `[📎N]` the
	 *  user can move next to what they say about it, or a masked paste's own
	 *  name. */
	add(incoming: File[], mode: FileTokenMode = 'clip'): void {
		if (!this.#o.enabled() || this.uploading) return;
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
	}

	remove(name: string): void {
		const before = this.files.map((f) => f.name);
		this.files = removeFileByName(this.files, name);
		const text = this.#o.input();
		const next = renumberClipTokens(text, before, this.files.map((f) => f.name));
		if (next !== text) this.#o.setInput(next);
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
		const paste = maskedPaste(text, this.files, this.#o.input(), this.#o.stagedNames());
		if (!paste) return;
		e.preventDefault();
		this.add([paste], 'name');
		toasts.ok(m.composer_large_paste({ name: paste.name, lines: text.split('\n').length }));
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
			const expanded = expandClipTokens(text, this.files, paths);
			const prose = prefixImageTokens(rewriteFileTokens(expanded, this.files, paths), this.files, paths);
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
