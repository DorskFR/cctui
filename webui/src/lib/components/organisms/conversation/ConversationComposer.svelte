<script lang="ts">
	import { tick, untrack } from 'svelte';
	import type { FollowupReason } from '@bindings/FollowupReason';
	import type { FollowupResponse } from '@bindings/FollowupResponse';
	import type { SendFollowup } from '@bindings/SendFollowup';
	import { insertBlock } from './insertText';
	import { splitDictated } from '#lib/voice/dictatedMark.ts';
	import { insertAtCaret, quoteMarkdown } from './format';
	import { errMessage } from '#lib/api.ts';
	import type { SessionListItem } from '@bindings/SessionListItem';
	import AttachmentList from '#lib/components/molecules/AttachmentList.svelte';
	import PromptField from '#lib/components/organisms/PromptField.svelte';
	import {
		endpoints,
		pendingScheduled,
		useScheduledActions,
		useScheduledMessages,
		useSessionAttachments
	} from '#lib/queries/index.ts';
	import {
		Button,
		FileButton,
		IconButton,
		InputGroup,
		Text,
		formatTimestamp,
		type MenuItem
	} from '@dorsk/tsumikit';
	import ArchivedActions from './ArchivedActions.svelte';
	import ScheduleCustomModal from './ScheduleCustomModal.svelte';
	import ScheduledMessages from './ScheduledMessages.svelte';
	import RedactedChips from './RedactedChips.svelte';
	import SpeechCapsHint from './SpeechCapsHint.svelte';
	import { wrapRedacted } from '#lib/redactTag.ts';
	import SendButton from './SendButton.svelte';
	import VoiceControls from './VoiceControls.svelte';
	import { CacheColdClock } from './cacheCold.svelte';
	import { followupDue } from './contextMeter';
	import { PromptAttachments } from '#lib/promptAttachments.svelte.ts';
	import { filePickGuard } from '#lib/filePickGuard.ts';
	import { scheduleBody } from './scheduleBody';
	import { coldSendItems, scheduleMenuItems } from './scheduleMenu';
	import { parseCustom, toLocalInput } from './scheduleTimes';
	import { drafts, composerKey, history as msgHistory } from '#lib/drafts.ts';
	import { serverDrafts } from '#lib/serverDrafts.ts';
	import { HistoryNav } from '#lib/historyNav.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import type { ScrollController } from './scroll.svelte';
	import { m } from '#lib/paraglide/messages.js';
	import { settings } from '#lib/settings.svelte.ts';
	import { launchFollowup, routeEnter, showColdOffer } from '#lib/followup.ts';
	import { hasFeature } from '#lib/features.svelte.ts';
	import { composerClearance } from '#lib/composerClearance.ts';

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
		onfollowupstarted,
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
		onsend: (body: string, dictated?: boolean, followup?: SendFollowup) => void;
		// Upload staged attachments, returning their absolute paths.
		stageFiles: (files: File[]) => Promise<{ paths: string[] }>;
		onNewFromScript: () => void;
		onFork?: () => void;
		onResume?: () => void;
		onFollowup?: (instruction?: string) => void;
		/** The server took a follow-up of this session. */
		onfollowupstarted?: (res: FollowupResponse, instruction: string, reason: FollowupReason) => void;
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

	// The pane remounts the composer per session, so the draft is seeded once.
	// svelte-ignore state_referenced_locally
	let input = $state(drafts.get(composerKey(session.id)));
	let revealed = $state<ReadonlySet<string>>(new Set());
	$effect(() => {
		drafts.set(composerKey(session.id), input);
	});
	let dictatedDraft = $state(false);
	$effect(() => {
		if (!input.trim()) dictatedDraft = false;
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
	const pickGuard = filePickGuard(() => toasts.error(m.attach_pick_lost()));
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

	// ── Scheduled send ───────────────────────────────────────────
	const schedulingOn = $derived(hasFeature('scheduling'));
	const scheduled = useScheduledMessages(() => session.id);
	const scheduledActions = useScheduledActions(() => session.id);
	const scheduledCount = $derived(pendingScheduled(scheduled.data).length);
	let scheduledEl = $state<HTMLElement>();
	let customOpen = $state(false);
	let customValue = $state('');
	const canSchedule = $derived(
		(!!input.trim() || att.files.length > 0) && !att.uploading && att.images.pending.length === 0
	);
	let launching = $state(false);
	// What Send / Enter does right now. A cold Send starts a follow-up only
	// where this pane can open one.
	const route = $derived(
		onFollowup ? routeEnter(settings.followupWhenCold, cache.cold) : 'send'
	);
	const coldItems = $derived(
		route === 'send'
			? null
			: { burstTokens, canSend: canSchedule && !launching, onsendhere: sendHere, onoptions: followup }
	);
	// A cold route already follows up; this labels the server's context rule on a plain send.
	const autoFollowupTokens = $derived(
		route === 'send' && !archived && followupDue(session) ? (session.context_tokens ?? null) : null
	);
	const autoItems = $derived<MenuItem[]>(
		autoFollowupTokens === null
			? []
			: [
					{
						id: 'send-here',
						label: m.autofollowup_send_here(),
						icon: 'send',
						disabled: !canSchedule,
						onselect: sendHere
					}
				]
	);
	const scheduleItems = $derived.by(() => {
		if (!schedulingOn) return coldItems ? coldSendItems(coldItems) : autoItems;
		const items = scheduleMenuItems({
			now: cache.now,
			canSchedule,
			scheduledCount,
			onpreset: (at) => void scheduleAt(at),
			oncustom: () => {
				customValue = toLocalInput(new Date(Date.now() + 3_600_000));
				customOpen = true;
			},
			onlist: () => scheduledEl?.scrollIntoView({ block: 'nearest', behavior: 'smooth' }),
			cold: coldItems
		});
		return [...autoItems, ...items];
	});

	async function scheduleAt(at: Date) {
		const text = input.trim();
		if (!schedulingOn || (!text && !att.files.length) || archived || att.uploading) return;
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
		clearSent(text);
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

	/** Start the follow-up with the draft as its instruction. The server
	 *  archives this session once the new one registers; until then the draft
	 *  can come back through `restoreDraft`. */
	async function launch() {
		const text = input.trim();
		if (launching || archived || !text || att.uploading || att.images.pending.length) return;
		// Staged files belong to this session's upload dir: the modal path keeps them.
		if (att.files.length) {
			followup();
			return;
		}
		launching = true;
		const source = session.id;
		const reason: FollowupReason = 'cold';
		try {
			const out = await launchFollowup(source, text, reason, endpoints.followup);
			if (out.status === 'in_progress') {
				toasts.info(m.followup_thread_in_progress());
				return;
			}
			if (out.status === 'failed') {
				toasts.error(
					m.spawn_toast_spawn_failed({
						error: out.error == null ? m.spawn_error_unknown() : errMessage(out.error)
					})
				);
				return;
			}
			clearSent(text, source);
			onfollowupstarted?.(out.followup, text, reason);
		} finally {
			launching = false;
		}
	}

	/** Put back an instruction whose follow-up failed, ahead of anything typed since. */
	export function restoreDraft(text: string) {
		const typed = input.trim();
		input = typed ? `${text}\n\n${input}` : text;
		resetHistoryNav();
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
		const draft = splitDictated(text);
		input = draft.text;
		dictatedDraft = draft.dictated;
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
		if (settings.voice.autoSend && route === 'send') return sendText(text, true);
		dictatedDraft = true;
		await insertText(text);
		if (settings.voice.autoSend) submit();
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
	export function sendText(text: string, dictated = false) {
		const body = text.trim();
		if (!body || archived) return;
		onsend(body, dictated);
		msgHistory.push(session.id, body);
	}

	function sendHere() {
		void send('never');
	}

	async function send(followup: SendFollowup = 'auto') {
		const text = input.trim();
		// Attachments alone are a valid message (the staged paths become the body).
		if ((!text && att.files.length === 0) || archived || att.uploading || att.images.pending.length)
			return;
		if (att.error) {
			toasts.error(att.error);
			return;
		}
		const body = await att.stage(text, stageFiles);
		if (body !== null) sendBody(body, followup);
	}

	// Hand the final body off to the parent's send orchestration, then clear the
	// composer + record the sent message in history.
	function sendBody(text: string, followup: SendFollowup) {
		if (!text || archived) return;
		onsend(text, dictatedDraft, followup);
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
	{@attach composerClearance}
	{@attach pickGuard}
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
		{#if schedulingOn}
			<div class="scheduled" class:hidden={folded} bind:this={scheduledEl}><ScheduledMessages sessionId={session.id} {archived} /></div>
		{/if}
		{#if supportsAttachments && (att.files.length || att.images.pending.length)}
			<div class="attachments">
				<AttachmentList {att} />
			</div>
		{/if}
		{#if coldOffer && !folded}
			<div class="cold-offer" data-testid="cold-offer">
				<Text tone="muted" size="sm">
					{#if route === 'launch'}
						{m.composer_followup_auto_offer()}
						<Button variant="link" size="sm" onclick={sendHere}
							>{m.composer_followup_send_here_link()}</Button
						>
					{:else}
						{m.composer_followup_offer()}
						<Button variant="link" size="sm" onclick={followup}>{m.composer_followup_start()}</Button>
					{/if}
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
		{#if !folded}
			<RedactedChips text={input} bind:revealed />
			<SpeechCapsHint />
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
					{autoFollowupTokens}
					{route}
					{launching}
					menu={schedulingOn || coldItems !== null || autoFollowupTokens !== null}
					armed={focused || !!input.trim() || att.files.length > 0}
					onclick={submit}
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
