<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import AttachmentList from '$lib/components/molecules/AttachmentList.svelte';
	import { Button, FileButton, Text, Textarea } from '@dorsk/tsumikit';
	import { drafts, composerKey, history as msgHistory } from '$lib/drafts';
	import { mergeFiles, removeFileByName, fileCapError } from '$lib/attachments';
	import { compact } from '$lib/format';
	import { toasts } from '$lib/toast.svelte';
	import type { ScrollController } from './scroll.svelte';

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
	let input = $state(drafts.get(composerKey(session.id)));
	$effect(() => {
		drafts.set(composerKey(session.id), input);
	});

	// ── Mid-chat file attachments (CCT-236) ────────────────────────────────
	// Held client-side next to the draft (File handles can't be persisted to
	// localStorage). On send we upload first, then append the staged paths under
	// the message text so the agent reads them.
	let attachments = $state<File[]>([]);
	let uploading = $state(false);
	let dragActive = $state(false);
	const attachError = $derived(fileCapError(attachments));
	export function addFiles(incoming: File[]) {
		if (!supportsAttachments || archived) return;
		attachments = mergeFiles(attachments, incoming);
	}
	export function setDragActive(active: boolean) {
		dragActive = active;
	}
	const removeAttachment = (name: string) => (attachments = removeFileByName(attachments, name));

	// Mask a large pasted block (CCT-297 #13): instead of dumping thousands of
	// characters into the composer, collapse it into a `paste-N.txt` attachment
	// (the Claude Code trick), keeping the textarea readable.
	const PASTE_MASK_CHARS = 2000;
	let pasteCounter = 1;
	let clipboardCounter = 1;

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
	function extForType(type: string): string {
		if (MIME_EXT[type]) return MIME_EXT[type];
		const sub = type.split('/')[1]?.split(';')[0];
		return sub ? sub.replace(/[^a-z0-9]/gi, '') || 'bin' : 'bin';
	}
	// Give a clipboard blob a stable, unique filename if it has none (pasted
	// screenshots/images arrive nameless), so dedupe-by-name doesn't collapse them.
	function namedClipboardFile(f: File): File {
		if (f.name && f.name.trim()) return f;
		const ext = extForType(f.type || 'application/octet-stream');
		return new File([f], `clipboard-${clipboardCounter++}.${ext}`, {
			type: f.type || 'application/octet-stream'
		});
	}
	// Extract binary files from a paste (copied files OR a pasted image/screenshot).
	// Prefer `items` (some browsers expose pasted screenshots only there, not in
	// `.files`), then fall back to `.files`.
	function clipboardBinaryFiles(cd: DataTransfer): File[] {
		const out: File[] = [];
		for (const item of Array.from(cd.items ?? [])) {
			if (item.kind !== 'file') continue;
			const f = item.getAsFile();
			if (f) out.push(namedClipboardFile(f));
		}
		if (out.length === 0) {
			for (const f of Array.from(cd.files ?? [])) out.push(namedClipboardFile(f));
		}
		return out;
	}

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
					? `Attached ${files[0].name}`
					: `Attached ${files.length} files from clipboard`
			);
			return;
		}
		const text = cd.getData('text/plain');
		if (!text || text.length < PASTE_MASK_CHARS) return; // small → normal paste
		e.preventDefault();
		const name = `paste-${pasteCounter++}.txt`;
		addFiles([new File([text], name, { type: 'text/plain' })]);
		const lines = text.split('\n').length;
		toasts.ok(`Large paste attached as ${name} (${lines} lines)`);
	}

	// ── Cold-cache Send button (CCT-189) ───────────────────────────────────
	// Anthropic's prompt cache is a ~5-min sliding window; once it lapses the next
	// send re-writes the whole context to cache (an expensive "burst"). The
	// button's "cold now" is purely time-based.
	const CACHE_TTL_MS = 5 * 60 * 1000;
	// Final-minute countdown window (CCT-261).
	const COLD_WARN_MS = 60 * 1000;
	let now = $state(Date.now());
	const lastActivityMs = $derived(
		session.last_activity_at ? new Date(session.last_activity_at).getTime() : null
	);
	// The cache window is anchored to the last FINISHED turn (CCT-279 item 2).
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

	// Pull a still-pending message back into the composer to edit + resend (CCT-208).
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
			toasts.err(attachError);
			return;
		}
		// Stage any pending attachments first; append the staged absolute paths under
		// the message so the agent reads them (CCT-236). On failure keep the draft +
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
				toasts.err(`Attachment upload failed: ${(e as Error).message}`);
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
			<div class="hint"><Text tone="muted" size="sm">Session archived (read-only).</Text></div>
			<span class="archived-actions-btns">
				<Button onclick={onNewFromScript}>New from same script</Button>
				<Button onclick={onFork}>Fork</Button>
				<Button variant="primary" onclick={onResume}>Resume</Button>
			</span>
		</div>
	{:else}
		<!-- Failed sends surface inline on the message bubble itself (red + Retry,
		     CCT-212), so there's no separate composer banner. -->
		{#if supportsAttachments && attachments.length}
			<div class="attachments">
				<AttachmentList files={attachments} onremove={removeAttachment} compact />
			</div>
		{/if}
		<div class="composer-row">
			{#if supportsAttachments}
				<!-- File picker (CCT-236). Drag-and-drop onto the conversation pane also
				     adds attachments. Icon-only: the label is hidden (a11y-only) so the
				     control stays a compact square matching the textarea/Send height. -->
				<FileButton label="Attach files" multiple iconOnly onfiles={addFiles} />
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
						? 'Drop files to attach'
						: coarsePointer
							? 'Message…'
							: 'Message… (Enter to send)'}
					bind:value={input}
					bind:el={scroll.textarea}
					onkeydown={onKey}
					oninput={() => resetHistoryNav()}
					onpaste={onPaste}
				/>
			</div>
			<!-- Stays a plain primary button across all cost states: layering a `tone`
			     (info/warn) on `primary` recolored the LABEL to the tone hue over the
			     accent fill (e.g. light-blue text on the green accent → unreadable,
			     CCT-349). The cold/imminent state is signalled by the label itself
			     (countdown · ❄️ · burst estimate) + the title tooltip, so the button
			     keeps its expected high-contrast primary colors. -->
			<Button
				variant="primary"
				class="send"
				disabled={uploading || (!input.trim() && attachments.length === 0)}
				onclick={send}
				title={cacheCold
					? burstTokens
						? `Prompt cache is cold — the next send re-writes ~${compact(burstTokens)} tokens to cache`
						: 'Prompt cache is cold — the next send re-bills the full context'
					: coldImminent
						? 'Prompt cache goes cold soon — send now to keep it warm'
						: undefined}
			>
				{#if uploading}Uploading…{:else if coldImminent}Send (<span class="countdown"
						>{coldCountdownSecs}s</span
					>){:else if cacheCold && burstTokens}Send ❄️ ~{compact(burstTokens)}{:else if cacheCold}Send
					❄️{:else}Send{/if}
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
	   (CCT-236). */
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
		   textarea so all three share a baseline at every font scale (CCT-279
		   item 8). */
		align-items: flex-end;
		/* Never let the row exceed the composer width — nowrap + min-width:0 on the
		   textarea keeps it contained. */
		min-width: 0;
		/* Single-row control height, tracking the Textarea's font-scaled single line
		   so Send + the attach FileButton stay the same height as a rows=1 input at
		   every font scale (CCT-353). The buttons' own floors are a fixed 2.5rem, but
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
	   with no way to reach Send (CCT-473). The autoresize action grows the
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
	/* Right-aligned action cluster: New from same script · Fork · Resume (CCT-345). */
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
	   which clashed with the primary fill (CCT-189/CCT-261/CCT-349). */
	.composer-row :global(.send),
	.composer-row :global(.file-btn) {
		flex: none;
		/* Track the font-scaled single-row height so all three composer controls
		   (attach · textarea · Send) stay level at every scale (CCT-353). */
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
