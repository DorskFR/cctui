<script lang="ts">
	import { tick, untrack } from 'svelte';
	import { insertBlock } from './insertText';
	import { insertAtCaret, quoteMarkdown } from './format';
	import type { SessionListItem } from '@bindings/SessionListItem';
	import AttachmentList from '$lib/components/molecules/AttachmentList.svelte';
	import PromptField from '$lib/components/organisms/PromptField.svelte';
	import { useSessionActions, useSessionAttachments } from '$lib/queries';
	import { FileButton, InputGroup } from '@dorsk/tsumikit';
	import ArchivedActions from './ArchivedActions.svelte';
	import ColdOffer from './ColdOffer.svelte';
	import ScheduleCustomModal from './ScheduleCustomModal.svelte';
	import ScheduledMessages from './ScheduledMessages.svelte';
	import RedactedChips from './RedactedChips.svelte';
	import { wrapRedacted } from '$lib/redactTag';
	import SendButton from './SendButton.svelte';
	import VoiceControls from './VoiceControls.svelte';
	import { CacheColdClock } from './cacheCold.svelte';
	import { PromptAttachments } from '$lib/promptAttachments.svelte';
	import { ComposerSchedule } from './composerSchedule.svelte';
	import { launchFromComposer } from './composerLaunch';
	import { scheduleMenuItems } from './scheduleMenu';
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
		lastAgentAt,
		supportsAttachments,
		scroll,
		onsend,
		stageFiles,
		onNewFromScript,
		onFork,
		onResume,
		onFollowup,
		compact = false,
		voiceEnabled = false,
		voiceLive = false,
		onvoicelive
	}: {
		session: SessionListItem;
		archived: boolean;
		/** Newest agent event this pane saw stream in, in ms. */
		lastAgentAt: number | null;
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
		/** The instance transcribes speech: show dictation. */
		voiceEnabled?: boolean;
		voiceLive?: boolean;
		/** Voice mode is available: show its toggle. */
		onvoicelive?: () => void;
	} = $props();

	let focused = $state(false);
	// Folded: a tile's composer before it takes focus — the input line alone.
	const folded = $derived(compact && !focused);

	// Composer draft, persisted per session in localStorage. Initialized once (the
	// drawer instance persists across session switches; matching the original we do
	// NOT reload input on switch — only the history-nav cursor resets, below).
	// svelte-ignore state_referenced_locally
	let input = $state(drafts.get(composerKey(session.id)));
	let revealed = $state<ReadonlySet<string>>(new Set());
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
		lastAgentAt: () => lastAgentAt
	});
	const burstTokens = $derived(session.estimated_burst_tokens ?? null);

	const schedule = new ComposerSchedule({
		sessionId: () => session.id,
		draft: () => input,
		archived: () => archived,
		att,
		stageFiles: (files) => stageFiles(files),
		restore: (text) => (input = text),
		sent: (text) => clearSent(text)
	});
	let scheduledEl = $state<HTMLElement>();
	const actions = useSessionActions();
	let launching = $state(false);
	// What Send / Enter does right now. A cold Send starts a follow-up only
	// where this pane can open one.
	const route = $derived(
		onFollowup ? routeEnter(settings.followupWhenCold, cache.cold) : 'send'
	);
	const scheduleItems = $derived(
		scheduleMenuItems({
			now: cache.now,
			canSchedule: schedule.canSchedule,
			scheduledCount: schedule.count,
			onpreset: (at) => void schedule.at(at),
			oncustom: () => schedule.openCustom(),
			onlist: () => scheduledEl?.scrollIntoView({ block: 'nearest', behavior: 'smooth' }),
			cold:
				route === 'send'
					? null
					: {
							burstTokens,
							canSend: schedule.canSchedule && !launching,
							onsendhere: send,
							onoptions: followup
						}
		})
	);

	let coldOfferDismissed = $state<string | null>(null);
	const coldOffer = $derived(
		!!onFollowup &&
			showColdOffer(settings.followupWhenCold, cache.cold, coldOfferDismissed === session.id)
	);
	function followup() {
		onFollowup?.(input.trim() || undefined);
	}
	function submit() {
		if (route === 'launch') void launch();
		else if (route === 'followup') followup();
		else void send();
	}

	function clearSent(text: string, id = session.id) {
		if (text) msgHistory.push(id, text);
		input = '';
		revealed = new Set();
		resetHistoryNav();
		drafts.clear(composerKey(id));
	}

	/** Start the follow-up with the draft as its instruction and archive this
	 *  session. On failure nothing is archived and the draft stays. */
	async function launch() {
		const text = input.trim();
		if (launching || archived || !text || att.uploading || att.images.pending.length) return;
		// Staged files belong to this session's upload dir: the modal path keeps them.
		if (att.files.length) {
			followup();
			return;
		}
		launching = true;
		const source = session;
		try {
			if (await launchFromComposer(source, text, actions)) clearSent(text, source.id);
		} finally {
			launching = false;
		}
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

	/** A finished dictation: into the draft, or out the way Send would route it
	 *  when the pref says so, so a cold session gets its follow-up. */
	async function dictated(text: string) {
		if (!settings.voice.autoSend) return void insertText(text);
		if (route === 'send') return sendText(text);
		await insertText(text);
		submit();
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
		clearSent(text);
	}

	// On touch/mobile, a bare Enter should insert a newline (the on-screen
	// keyboard's return key is easy to hit by accident) — send only via the Send
	// button or Ctrl/Cmd+Enter. On desktop, Enter sends and Shift+Enter newlines.
	const coarsePointer =
		typeof window !== 'undefined' &&
		typeof window.matchMedia === 'function' &&
		window.matchMedia('(pointer: coarse)').matches;

	async function wrapSelection() {
		const el = scroll.textarea;
		if (!el) return;
		const next = wrapRedacted(input, el.selectionStart, el.selectionEnd);
		input = next.text;
		resetHistoryNav();
		await tick();
		el.setSelectionRange(next.selStart, next.selEnd);
	}

	// Plain Enter is the Textarea's `submitOn`; only history nav, the
	// always-available mod+Enter chord and the redact wrap are handled here.
	function onKey(e: KeyboardEvent) {
		if (e.key.toLowerCase() === 'r' && e.shiftKey && (e.ctrlKey || e.metaKey) && !e.altKey) {
			e.preventDefault();
			void wrapSelection();
			return;
		}
		if (nav.handleKey(e)) return;
		if (e.key === 'Enter' && !coarsePointer && (e.ctrlKey || e.metaKey)) {
			e.preventDefault();
			submit();
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
			<ColdOffer
				{route}
				onsendhere={() => void send()}
				onfollowup={followup}
				ondismiss={() => (coldOfferDismissed = session.id)}
			/>
		{/if}
		{#if !folded}
			<RedactedChips text={input} bind:revealed />
		{/if}
		{#snippet leadingTools()}
			{#if supportsAttachments}
				<FileButton
					label={m.composer_attach_files()}
					multiple
					box="sm"
					iconOnly
					variant="ghost"
					onfiles={addFiles}
				/>
			{/if}
			{#if voiceEnabled}
				<VoiceControls live={voiceLive} ontogglelive={onvoicelive} ondictated={dictated} />
			{/if}
		{/snippet}
		<!-- The `#` session-mention panel opens as a dropup above the field
		     (the composer is pinned to the bottom of the drawer). -->
		<InputGroup leading={(supportsAttachments || voiceEnabled) && !folded ? leadingTools : undefined}>
			<PromptField
				att={supportsAttachments ? att : undefined}
				bind:value={input}
				bind:el={scroll.textarea}
				excludeId={session.id}
				{revealed}
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
					{route}
					{launching}
					active={focused || !!input.trim() || att.files.length > 0}
					onclick={submit}
				/>
			{/snippet}
		</InputGroup>
	{/if}
</div>

{#if schedule.customOpen}
	<ScheduleCustomModal
		now={cache.now}
		bind:value={schedule.customValue}
		onconfirm={() => schedule.confirmCustom()}
		onclose={() => (schedule.customOpen = false)}
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
	.attachments {
		width: 100%;
	}
</style>
