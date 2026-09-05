<script lang="ts">
	import { errMessage } from '$lib/api';
	import type { SessionListItem } from '@bindings/SessionListItem';
	import AttachmentList from '$lib/components/molecules/AttachmentList.svelte';
	import { Button, FileButton, Text, Textarea } from '@dorsk/tsumikit';
	import { drafts, composerKey, history as msgHistory } from '$lib/drafts';
	import {
		attachFiles,
		nextPasteIndex,
		removeFileByName,
		fileCapError,
		makeClipboardFiles
	} from '$lib/attachments';
	import { attachmentStore, dropMissingTokens } from '$lib/attachmentStore';
	import { compact } from '$lib/format';
	import { toasts } from '$lib/toast.svelte';
	import type { ScrollController } from './scroll.svelte';
	import { cacheTtlMs } from './cacheTtl';
	import { m } from '$lib/paraglide/messages';

	let {
		session,
		archived,
		working,
		supportsAttachments,
		scroll,
		onsend,
		stageFiles,
		onNewFromScript,
		onFork,
		onResume
	}: {
		session: SessionListItem;
		archived: boolean;
		working: boolean;
		supportsAttachments: boolean;
		scroll: ScrollController;
		// Send a final message body (text + any appended staged-attachment paths).
		onsend: (body: string) => void;
		// Upload staged attachments, returning their absolute paths.
		stageFiles: (files: File[]) => Promise<{ paths: string[] }>;
		onNewFromScript: () => void;
		onFork: () => void;
		onResume: () => void;
	} = $props();

	// Composer draft, persisted per session in localStorage. Initialized once (the
	// drawer instance persists across session switches; matching the original we do
	// NOT reload input on switch — only the history-nav cursor resets, below).
	// svelte-ignore state_referenced_locally
	let input = $state(drafts.get(composerKey(session.id)));
	$effect(() => {
		drafts.set(composerKey(session.id), input);
	});

	// ── Mid-chat file attachments ────────────────────────────────
	// Persisted per session in IndexedDB next to the localStorage draft. On send
	// we upload first, then append the staged paths under the message text so
	// the agent reads them.
	let attachments = $state<File[]>([]);
	// Key of the session whose attachments are loaded; null while a restore is
	// in flight so a session switch never writes the old list under the new key.
	let attachmentsKey = $state<string | null>(null);
	$effect(() => {
		if (!attachmentsKey) return;
		void attachmentStore.set(attachmentsKey, [...attachments]);
	});
	$effect(() => {
		const key = composerKey(session.id);
		attachmentsKey = null;
		let live = true;
		(async () => {
			const restored = await attachmentStore.get(key);
			if (!live) return;
			attachments = restored.files;
			const { text, dropped } = dropMissingTokens(input, restored.missing);
			if (dropped) {
				input = text;
				toasts.info(m.attachments_missing_dropped({ count: dropped }));
			}
			attachmentsKey = key;
		})();
		return () => {
			live = false;
		};
	});
	let uploading = $state(false);
	let dragActive = $state(false);
	const attachError = $derived(fileCapError(attachments));
	export function addFiles(incoming: File[]) {
		if (!supportsAttachments || archived) return;
		({ files: attachments, text: input } = attachFiles(attachments, input, incoming));
	}
	export function setDragActive(active: boolean) {
		dragActive = active;
	}
	const removeAttachment = (name: string) => (attachments = removeFileByName(attachments, name));

	// Mask a large pasted block: instead of dumping thousands of
	// characters into the composer, collapse it into a `paste-N.txt` attachment
	// (the Claude Code trick), keeping the textarea readable. The index is derived
	// from current attachments + draft tokens: the composer remounts on drawer
	// close while the draft (and its `[paste-N.txt]`) persists per session.
	const PASTE_MASK_CHARS = 2000;
	const clipboardBinaryFiles = makeClipboardFiles();

	function onPaste(e: ClipboardEvent) {
		if (!supportsAttachments || archived) return;
		const cd = e.clipboardData;
		if (!cd) return;
		// Binary clipboard content (pasted screenshot/image or copied file) → attach
		// it via the same staged-upload path as the 📎 picker and drag-and-drop.
		const files = clipboardBinaryFiles(cd);
		if (files.length > 0) {
			e.preventDefault();
			addFiles(files);
			toasts.ok(
				files.length === 1
					? m.composer_attached_file({ name: files[0].name })
					: m.composer_attached_files_clipboard({ count: files.length })
			);
			return;
		}
		const text = cd.getData('text/plain');
		if (!text || text.length < PASTE_MASK_CHARS) return; // small → normal paste
		e.preventDefault();
		const name = `paste-${nextPasteIndex(attachments, input)}.txt`;
		addFiles([new File([text], name, { type: 'text/plain' })]);
		const lines = text.split('\n').length;
		toasts.ok(m.composer_large_paste({ name, lines }));
	}

	// ── Cold-cache Send button ───────────────────────────────────
	// Once the prompt cache lapses the next send re-writes the whole context to
	// cache (an expensive "burst"). The button's "cold now" is purely time-based.
	// The TTL window is provider/family- and model-dependent: Anthropic
	// 60m, OpenAI GPT-5.6+ 30m, else the 5-min legacy sliding window.
	const CACHE_TTL_MS = $derived(cacheTtlMs(session.adapter_id, session.model ?? null));
	// Final-minute countdown window.
	const COLD_WARN_MS = 60 * 1000;
	let now = $state(Date.now());
	const lastActivityMs = $derived(
		session.last_activity_at ? new Date(session.last_activity_at).getTime() : null
	);
	// The cache window is anchored to the last FINISHED turn.
	// While a turn is in flight (`working`) suppress the cold/countdown UI so it
	// can't flip "cold" mid-turn; it re-anchors off the new reply once the turn ends.
	const cacheCold = $derived(
		!working && lastActivityMs !== null && now - lastActivityMs > CACHE_TTL_MS
	);
	const burstTokens = $derived(session.estimated_burst_tokens ?? null);
	const msUntilCold = $derived(
		lastActivityMs === null ? null : CACHE_TTL_MS - (now - lastActivityMs)
	);
	const coldImminent = $derived(
		!working && msUntilCold !== null && msUntilCold > 0 && msUntilCold <= COLD_WARN_MS
	);
	const coldCountdownSecs = $derived(coldImminent ? Math.ceil(msUntilCold! / 1000) : null);
	// Tick fast (1s) only while counting down; otherwise a lazy 15s tick is enough
	// to flip the button cold.
	$effect(() => {
		const fast = coldImminent;
		const t = setInterval(() => (now = Date.now()), fast ? 1_000 : 15_000);
		return () => clearInterval(t);
	});

	// ── Sent-message history recall (ArrowUp/ArrowDown) ─────────────────────
	// histIndex: -1 = editing the live draft; 0..n-1 = browsing history (newest-
	// first as you press Up). draftStash holds the in-progress text so returning
	// past the newest entry restores it.
	let histIndex = $state(-1);
	let draftStash = '';
	function resetHistoryNav() {
		histIndex = -1;
	}
	// Reset the history-nav cursor when the open session changes (matching the
	// original drawer's subscribe-effect reset).
	$effect(() => {
		void session.id;
		histIndex = -1;
		draftStash = '';
	});

	// Pull a still-pending message back into the composer to edit + resend.
	export function loadDraft(text: string) {
		input = text;
		resetHistoryNav();
		scroll.textarea?.focus();
	}

	async function send() {
		const text = input.trim();
		// Allow sending attachments with no text (the staged paths become the
		// message), but require at least one of text/attachments.
		if ((!text && attachments.length === 0) || archived || uploading) return;
		if (attachError) {
			toasts.error(attachError);
			return;
		}
		// Stage any pending attachments first; append the staged absolute paths under
		// the message so the agent reads them. On failure keep the draft +
		// attachments intact and surface the error rather than sending a half-message.
		let body = text;
		if (attachments.length) {
			uploading = true;
			try {
				const { paths } = await stageFiles(attachments);
				const list = paths.map((p) => `- ${p}`).join('\n');
				const header = paths.length === 1 ? 'Attached file:' : `Attached files (${paths.length}):`;
				body = text ? `${text}\n\n${header}\n${list}` : `${header}\n${list}`;
				attachments = [];
			} catch (e) {
				toasts.error(m.composer_attachment_upload_failed({ message: errMessage(e) }));
				return;
			} finally {
				uploading = false;
			}
		}
		sendBody(body);
	}

	// Hand the final body off to the parent's send orchestration, then clear the
	// composer + record the sent message in history.
	function sendBody(text: string) {
		if (!text || archived) return;
		onsend(text);
		msgHistory.push(session.id, text);
		input = '';
		resetHistoryNav();
		drafts.clear(composerKey(session.id));
	}

	// On touch/mobile, a bare Enter should insert a newline (the on-screen
	// keyboard's return key is easy to hit by accident) — send only via the Send
	// button or Ctrl/Cmd+Enter. On desktop, Enter sends and Shift+Enter newlines.
	const coarsePointer =
		typeof window !== 'undefined' &&
		typeof window.matchMedia === 'function' &&
		window.matchMedia('(pointer: coarse)').matches;

	// True when the caret is at the very start of the textarea (so ArrowUp can
	// recall history without fighting normal multiline cursor movement).
	function caretAtStart(): boolean {
		const el = scroll.textarea;
		if (!el) return false;
		return el.selectionStart === 0 && el.selectionEnd === 0;
	}
	function caretAtEnd(): boolean {
		const el = scroll.textarea;
		if (!el) return false;
		return el.selectionStart === input.length && el.selectionEnd === input.length;
	}

	function historyBack() {
		const list = msgHistory.get(session.id);
		if (list.length === 0) return;
		if (histIndex === -1) draftStash = input; // stash live draft before browsing
		const next = Math.min(histIndex + 1, list.length - 1);
		histIndex = next;
		input = list[list.length - 1 - next]; // newest-first
	}
	function historyForward() {
		const list = msgHistory.get(session.id);
		if (histIndex === -1) return;
		const next = histIndex - 1;
		if (next < 0) {
			histIndex = -1;
			input = draftStash; // restored the in-progress draft
		} else {
			histIndex = next;
			input = list[list.length - 1 - next];
		}
	}

	function onKey(e: KeyboardEvent) {
		if (e.key === 'ArrowUp' && (histIndex !== -1 || caretAtStart())) {
			e.preventDefault();
			historyBack();
			return;
		}
		if (e.key === 'ArrowDown' && histIndex !== -1 && caretAtEnd()) {
			e.preventDefault();
			historyForward();
			return;
		}
		if (e.key !== 'Enter') return;
		if (e.ctrlKey || e.metaKey) {
			e.preventDefault();
			send();
			return;
		}
		if (!coarsePointer && !e.shiftKey) {
			e.preventDefault();
			send();
		}
	}
</script>

<div class="composer" class:dropping={dragActive}>
	{#if archived}
		<div class="archived-actions">
			<div class="hint"><Text tone="muted" size="sm">{m.composer_archived_readonly()}</Text></div>
			<span class="archived-actions-btns">
				<Button onclick={onNewFromScript}>{m.composer_new_from_script()}</Button>
				<Button onclick={onFork}>{m.composer_fork()}</Button>
				<Button variant="primary" onclick={onResume}>{m.composer_resume()}</Button>
			</span>
		</div>
	{:else}
		<!-- Failed sends surface inline on the message bubble itself (red +
		     Retry), so there's no separate composer banner. -->
		{#if supportsAttachments && attachments.length}
			<div class="attachments">
				<AttachmentList files={attachments} onremove={removeAttachment} compact />
			</div>
		{/if}
		<div class="composer-row">
			{#if supportsAttachments}
				<!-- File picker. Drag-and-drop onto the conversation pane also
				     adds attachments. Icon-only: the label is hidden (a11y-only) so the
				     control stays a compact square matching the textarea/Send height. -->
				<FileButton label={m.composer_attach_files()} multiple iconOnly onfiles={addFiles} />
			{/if}
			<!-- Starts at one row (Textarea's baked-in min-height) and grows with
			     content (autoresize). The top handle drags a min-height floor so
			     the user can pin a taller working area; content still grows past it
			     (tsumikit 0.2.15). -->
			<div class="composer-input">
				<Textarea
					rows={1}
					autoresize
					resize="top"
					placeholder={dragActive
						? m.composer_drop_files()
						: coarsePointer
							? m.composer_placeholder_message()
							: m.composer_placeholder_message_enter()}
					bind:value={input}
					bind:el={scroll.textarea}
					onkeydown={onKey}
					oninput={() => resetHistoryNav()}
					onpaste={onPaste}
				/>
			</div>
			<!-- Stays a plain primary button across all cost states: layering a `tone`
			     (info/warn) on `primary` recolored the LABEL to the tone hue over the
			     accent fill (e.g. light-blue text on the green accent → unreadable).
			     The cold/imminent state is signalled by the label itself
			     (countdown · ❄️ · burst estimate) + the title tooltip, so the button
			     keeps its expected high-contrast primary colors. -->
			<Button
				variant="primary"
				class="send"
				disabled={uploading || (!input.trim() && attachments.length === 0)}
				onclick={send}
				title={cacheCold
					? burstTokens
						? m.composer_cache_cold_burst({ tokens: compact(burstTokens) })
						: m.composer_cache_cold()
					: coldImminent
						? m.composer_cache_imminent()
						: undefined}
			>
				{#if uploading}{m.composer_uploading()}{:else if coldImminent}{m.composer_send()} (<span
						class="countdown">{coldCountdownSecs}s</span
					>){:else if cacheCold && burstTokens}{m.composer_send()} ❄️ ~{compact(
						burstTokens
					)}{:else if cacheCold}{m.composer_send()}
					❄️{:else}{m.composer_send()}{/if}
			</Button>
		</div>
	{/if}
</div>

<style>
	.composer {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-3);
		padding-bottom: calc(var(--sp-3) + var(--safe-bottom));
		border-top: 1px solid var(--border);
		background: var(--bg-elevated);
	}
	/* Highlight the composer while a file drag hovers the conversation pane
	  . */
	.composer.dropping {
		outline: 2px dashed var(--c-blue);
		outline-offset: -2px;
		background: color-mix(in srgb, var(--c-blue) 8%, var(--bg-elevated));
	}
	.composer-row {
		display: flex;
		flex-wrap: nowrap;
		gap: var(--sp-2);
		/* Align the attach/send controls to the BOTTOM edge of the (growable)
		   textarea so all three share a baseline at every font scale. */
		align-items: flex-end;
		/* Never let the row exceed the composer width — nowrap + min-width:0 on the
		   textarea keeps it contained. */
		min-width: 0;
		/* Single-row control height, tracking the Textarea's font-scaled single line
		   so Send + the attach FileButton stay the same height as a rows=1 input at
		   every font scale. The buttons' own floors are a fixed 2.5rem, but
		   the textarea grows with --fs-base (form-control font is max(16px,--fs-base))
		   while a 0.8125rem-label button does not — leaving Send shorter at the largest
		   scale. Mirror the Textarea's metrics: line-box (max(16px,--fs-base) ×
		   --lh-tight) + 2×--sp-2 vertical padding + 2px border, floored at 2.5rem. */
		--composer-ctl-h: max(
			2.5rem,
			calc(max(16px, var(--fs-base)) * var(--lh-tight) + 2 * var(--sp-2) + 2px)
		);
	}
	/* The Textarea now ships its own .textarea-wrap root, so the row flex lives on
	   this layout wrapper rather than the textarea element. */
	.composer-input {
		flex: 1;
		min-width: 0;
	}
	/* Cap the autoresizing textarea's growth so a long message can't push the
	   composer controls (textarea bottom + Send) off the bottom of the viewport
	   with no way to reach Send. The autoresize action grows the
	   element's inline `height`; `max-height` clamps it and the textarea scrolls
	   internally past the cap. 40vh keeps the conversation + composer visible at
	   any message length; the composer itself stays pinned as the last flex child
	   of the column-flex drawer. */
	.composer-input :global(.textarea) {
		max-height: 40vh;
		overflow-y: auto;
	}
	.attachments {
		width: 100%;
	}
	.archived-actions {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: var(--sp-2);
	}
	/* Right-aligned action cluster: New from same script · Fork · Resume. */
	.archived-actions-btns {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		margin-left: auto;
	}
	/* Send button is a child Button; keep it from shrinking in the flex row. Its
	   height comes from the Button atom's md size (2.5rem), matching the attach
	   FileButton and the collapsed Textarea. The cold + final-minute cost states
	   are conveyed by the button LABEL (countdown/❄️/burst) — not a `tone` recolor,
	   which clashed with the primary fill. */
	.composer-row :global(.send),
	.composer-row :global(.file-btn) {
		flex: none;
		/* Track the font-scaled single-row height so all three composer controls
		   (attach · textarea · Send) stay level at every scale. */
		min-height: var(--composer-ctl-h);
	}
	/* Fixed-width, tabular digits so "59s"→"0s" doesn't jitter the button. The
	   countdown <span> is in this component's markup, so a scoped rule reaches it. */
	.countdown {
		display: inline-block;
		min-width: 2.4ch;
		text-align: right;
		font-variant-numeric: tabular-nums;
	}
	.hint {
		text-align: center;
		width: 100%;
	}
</style>
