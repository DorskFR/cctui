<script lang="ts">
	import { tick, untrack } from 'svelte';
	import { insertBlock } from './insertText';
	import { insertAtCaret, quoteMarkdown } from './format';
	import { errMessage } from '$lib/api';
	import type { SessionListItem } from '@bindings/SessionListItem';
	import AttachmentList from '$lib/components/molecules/AttachmentList.svelte';
	import PromptField from '$lib/components/organisms/PromptField.svelte';
	import {
		pendingScheduled,
		useScheduledActions,
		useScheduledMessages,
		useSessionAttachments
	} from '$lib/queries';
	import { Button, FileButton, IconButton, InputGroup, Text, formatTimestamp } from '@dorsk/tsumikit';
	import ArchivedActions from './ArchivedActions.svelte';
	import ScheduleCustomModal from './ScheduleCustomModal.svelte';
	import ScheduledMessages from './ScheduledMessages.svelte';
	import SendButton from './SendButton.svelte';
	import { CacheColdClock } from './cacheCold.svelte';
	import { PromptAttachments } from '$lib/promptAttachments.svelte';
	import { scheduleBody } from './scheduleBody';
	import { scheduleMenuItems } from './scheduleMenu';
	import { parseCustom, toLocalInput } from './scheduleTimes';
	import { drafts, composerKey, history as msgHistory } from '$lib/drafts';
	import { serverDrafts } from '$lib/serverDrafts';
	import { HistoryNav } from '$lib/historyNav';
	import { toasts } from '$lib/toast.svelte';
	import type { ScrollController } from './scroll.svelte';
	import { m } from '$lib/paraglide/messages';
	import { settings } from '$lib/settings.svelte';
	import { routeEnter, showColdOffer } from '$lib/followup';

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
		onResume,
		onFollowup,
		compact = false
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
		onFollowup?: (instruction?: string) => void;
		/** Tile chrome: one line of input until it takes focus. */
		compact?: boolean;
	} = $props();

	let focused = $state(false);
	// Folded: a tile's composer before it takes focus — the input line alone.
	const folded = $derived(compact && !focused);

	// Composer draft, persisted per session in localStorage. Initialized once (the
	// drawer instance persists across session switches; matching the original we do
	// NOT reload input on switch — only the history-nav cursor resets, below).
	// svelte-ignore state_referenced_locally
	let input = $state(drafts.get(composerKey(session.id)));
	$effect(() => {
		drafts.set(composerKey(session.id), input);
	});

	// A draft written on another device lands after first paint (which seeds from
	// the local mirror), so adopt it only while the box still holds what we seeded.
	$effect(() => {
		const key = composerKey(session.id);
		const seeded = untrack(() => input);
		void serverDrafts.ready.then(() => {
			const roamed = drafts.get(key);
			if (roamed && untrack(() => input) === seeded) input = roamed;
		});
	});

	const stagedQuery = useSessionAttachments(
		() => session.id,
		() => supportsAttachments && !archived
	);
	const att = new PromptAttachments({
		draftKey: () => composerKey(session.id),
		enabled: () => supportsAttachments && !archived,
		input: () => input,
		setInput: (text) => (input = text),
		stagedNames: () => (stagedQuery.data ?? []).map((a) => a.name),
		el: () => scroll.textarea
	});
	export function addFiles(incoming: File[]) {
		att.add(incoming);
	}
	export function setDragActive(active: boolean) {
		att.setDragActive(active);
	}

	const cache = new CacheColdClock({
		adapterId: () => session.adapter_id,
		model: () => session.model ?? null,
		lastActivityAt: () => session.last_activity_at ?? null,
		working: () => working
	});
	const burstTokens = $derived(session.estimated_burst_tokens ?? null);

	// ── Scheduled send ───────────────────────────────────────────
	const scheduled = useScheduledMessages(() => session.id);
	const scheduledActions = useScheduledActions(() => session.id);
	const scheduledCount = $derived(pendingScheduled(scheduled.data).length);
	let scheduledEl = $state<HTMLElement>();
	let customOpen = $state(false);
	let customValue = $state('');
	const canSchedule = $derived(
		(!!input.trim() || att.files.length > 0) && !att.uploading && att.images.pending.length === 0
	);
	const scheduleItems = $derived(
		scheduleMenuItems({
			now: cache.now,
			canSchedule,
			scheduledCount,
			onpreset: (at) => void scheduleAt(at),
			oncustom: () => {
				customValue = toLocalInput(new Date(Date.now() + 3_600_000));
				customOpen = true;
			},
			onlist: () => scheduledEl?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
		})
	);

	async function scheduleAt(at: Date) {
		const text = input.trim();
		if ((!text && !att.files.length) || archived || att.uploading) return;
		if (att.error) {
			toasts.error(att.error);
			return;
		}
		const out = await scheduleBody(
			text,
			(t) => att.stage(t, stageFiles),
			(body) => scheduledActions.schedule(body, at)
		);
		if (!out.ok) {
			if (out.restore !== null) input = out.restore;
			if (out.error) toasts.error(m.composer_schedule_failed({ message: errMessage(out.error) }));
			return;
		}
		toasts.info(
			m.composer_schedule_toast({
				when: formatTimestamp(at, 'datetime')
			})
		);
		msgHistory.push(session.id, text);
		input = '';
		resetHistoryNav();
		drafts.clear(composerKey(session.id));
	}

	function scheduleCustom() {
		const at = parseCustom(customValue, new Date());
		if (!at) {
			toasts.error(m.composer_schedule_custom_invalid());
			return;
		}
		customOpen = false;
		void scheduleAt(at);
	}

	let coldOfferDismissed = $state<string | null>(null);
	const coldOffer = $derived(
		!!onFollowup &&
			showColdOffer(settings.followupWhenCold, cache.cold, coldOfferDismissed === session.id)
	);
	function followup() {
		onFollowup?.(input.trim() || undefined);
	}
	function submit() {
		if (onFollowup && routeEnter(settings.followupWhenCold, cache.cold, false) === 'followup') {
			followup();
			return;
		}
		send();
	}

	const nav = new HistoryNav({
		list: () => msgHistory.get(session.id),
		value: () => input,
		setValue: (v) => (input = v),
		el: () => scroll.textarea
	});
	function resetHistoryNav() {
		nav.reset();
	}
	$effect(() => {
		void session.id;
		nav.resetAll();
	});

	// Pull a still-pending message back into the composer to edit + resend.
	export function loadDraft(text: string) {
		input = text;
		resetHistoryNav();
		scroll.textarea?.focus();
	}

	export function focus() {
		scroll.textarea?.focus();
	}

	/** Drop a block at the caret (a plugin pane's context), keeping the draft. */
	export async function insertText(text: string) {
		const el = scroll.textarea;
		const caret = el && document.activeElement === el ? el.selectionStart : undefined;
		const next = insertBlock(input, text, caret);
		input = next.value;
		resetHistoryNav();
		await tick();
		if (!el) return;
		el.focus();
		el.setSelectionRange(next.caret, next.caret);
	}

	/** Quote a message into the draft as a Markdown blockquote, keeping whatever
	 *  is already typed and leaving the caret under the quote. */
	export async function insertQuote(text: string) {
		const block = quoteMarkdown(text);
		if (!block) return;
		const el = scroll.textarea;
		const caret = el && document.activeElement === el ? el.selectionStart : undefined;
		const next = insertAtCaret(input, caret, block);
		input = next.text;
		resetHistoryNav();
		await tick();
		if (!el) return;
		el.focus();
		el.setSelectionRange(next.caret, next.caret);
		el.scrollIntoView({ block: 'nearest' });
	}

	/** A plugin pane's message: goes out through the same path as a typed one,
	 *  the draft stays. */
	export function sendText(text: string) {
		const body = text.trim();
		if (!body || archived) return;
		onsend(body);
		msgHistory.push(session.id, body);
	}

	async function send() {
		const text = input.trim();
		// Attachments alone are a valid message (the staged paths become the body).
		if ((!text && att.files.length === 0) || archived || att.uploading || att.images.pending.length)
			return;
		if (att.error) {
			toasts.error(att.error);
			return;
		}
		const body = await att.stage(text, stageFiles);
		if (body !== null) sendBody(body);
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

	// Plain Enter is the Textarea's `submitOn`; only history nav and the
	// always-available mod+Enter chord are handled here.
	function onKey(e: KeyboardEvent) {
		if (nav.handleKey(e)) return;
		if (e.key === 'Enter' && !coarsePointer && (e.ctrlKey || e.metaKey)) {
			e.preventDefault();
			send();
			return;
		}
		if (
			e.key === 'Enter' &&
			e.shiftKey &&
			!coarsePointer &&
			onFollowup &&
			routeEnter(settings.followupWhenCold, cache.cold, false) === 'followup'
		) {
			e.preventDefault();
			send();
		}
	}
</script>

<div
	class="composer"
	data-journey="composer"
	class:dropping={att.dragActive}
	class:compact
	onfocusin={() => (focused = true)}
	onfocusout={() => (focused = false)}
>
	{#if archived}
		<ArchivedActions {onNewFromScript} {onFork} {onResume} />
	{:else}
		<!-- Failed sends surface inline on the message bubble itself (red +
		     Retry), so there's no separate composer banner. -->
		<div class="scheduled" class:hidden={folded} bind:this={scheduledEl}><ScheduledMessages sessionId={session.id} {archived} /></div>
		{#if supportsAttachments && (att.files.length || att.images.pending.length)}
			<div class="attachments">
				<AttachmentList {att} />
			</div>
		{/if}
		{#if coldOffer && !folded}
			<div class="cold-offer">
				<Text tone="muted" size="sm">
					{m.composer_followup_offer()}
					<Button variant="link" size="sm" onclick={followup}>{m.composer_followup_start()}</Button>
				</Text>
				<IconButton
					icon="x"
					variant="ghost"
					box="xs"
					label={m.composer_followup_dismiss()}
					onclick={() => (coldOfferDismissed = session.id)}
				/>
			</div>
		{/if}
		{#snippet attach()}
			<FileButton
				label={m.composer_attach_files()}
				multiple
				box="sm"
				iconOnly
				variant="ghost"
				onfiles={addFiles}
			/>
		{/snippet}
		<!-- The `#` session-mention panel opens as a dropup above the field
		     (the composer is pinned to the bottom of the drawer). -->
		<InputGroup leading={supportsAttachments && !folded ? attach : undefined}>
			<PromptField
				att={supportsAttachments ? att : undefined}
				bind:value={input}
				bind:el={scroll.textarea}
				excludeId={session.id}
				placement="up"
				rows={1}
				autoresize
				resize="top"
				maxHeight="40vh"
				submitOn={coarsePointer ? 'mod-enter' : 'enter'}
				onsubmit={submit}
				data-journey="message"
				aria-label={m.a11y_composer_message()}
				placeholder={coarsePointer
					? m.composer_placeholder_message()
					: m.composer_placeholder_message_enter()}
				onkeydown={onKey}
				oninput={() => resetHistoryNav()}
			/>
			{#snippet trailing()}
				<SendButton
					items={scheduleItems}
					disabled={att.uploading ||
						att.images.pending.length > 0 ||
						(!input.trim() && att.files.length === 0)}
					uploading={att.uploading}
					cacheCold={cache.cold}
					coldImminent={cache.imminent}
					coldCountdownSecs={cache.countdownSecs}
					{burstTokens}
					onclick={send}
				/>
			{/snippet}
		</InputGroup>
	{/if}
</div>

{#if customOpen}
	<ScheduleCustomModal
		now={cache.now}
		bind:value={customValue}
		onconfirm={scheduleCustom}
		onclose={() => (customOpen = false)}
	/>
{/if}

<style>
	.composer {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3) calc(var(--sp-3) + var(--safe-bottom));
		border-top: 1px solid var(--border);
		background: var(--bg-elevated);
	}
	@media (max-width: 959px) {
		.composer {
			padding: var(--sp-2) var(--sp-2) calc(var(--sp-2) + var(--safe-bottom));
		}
	}
	/* A tile's composer is one line of chrome until the user means to type. */
	.composer.compact {
		gap: var(--sp-1);
		padding: var(--sp-1) var(--sp-2) calc(var(--sp-1) + var(--safe-bottom));
	}
	.hidden {
		display: none;
	}
	.scheduled:empty {
		display: none;
	}
	/* Highlight the composer while a file drag hovers the conversation pane. */
	.composer.dropping {
		outline: 2px dashed var(--c-blue);
		outline-offset: -2px;
		background: color-mix(in srgb, var(--c-blue) 8%, var(--bg-elevated));
	}
	.cold-offer {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
	}
	.attachments {
		width: 100%;
	}
</style>
