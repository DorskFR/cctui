<script lang="ts">
	import { onDestroy } from 'svelte';
	import ImageCompressionStatus from '$lib/components/molecules/ImageCompressionStatus.svelte';
	import { errMessage } from '$lib/api';
	import type { SessionListItem } from '@bindings/SessionListItem';
	import AttachmentList from '$lib/components/molecules/AttachmentList.svelte';
	import SessionMention from '$lib/components/molecules/SessionMention.svelte';
	import {
		pendingScheduled,
		useScheduledActions,
		useScheduledMessages,
		useSessionAttachments,
		useSessions
	} from '$lib/queries';
	import { Button, FileButton, IconButton, InputGroup, Text, Textarea } from '@dorsk/tsumikit';
	import ArchivedActions from './ArchivedActions.svelte';
	import ScheduleCustomModal from './ScheduleCustomModal.svelte';
	import ScheduledMessages from './ScheduledMessages.svelte';
	import SendButton from './SendButton.svelte';
	import { CacheColdClock } from './cacheCold.svelte';
	import { ComposerAttachments } from './composerAttachments.svelte';
	import { scheduleMenuItems } from './scheduleMenu';
	import { parseCustom, toLocalInput } from './scheduleTimes';
	import { drafts, composerKey, history as msgHistory } from '$lib/drafts';
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
		onFollowup
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
	} = $props();

	// `#` mention popover source: the shared (cached) session list.
	const sessionsQuery = useSessions(() => false);
	const mentionSessions = $derived(sessionsQuery.data?.sessions ?? []);

	// Composer draft, persisted per session in localStorage. Initialized once (the
	// drawer instance persists across session switches; matching the original we do
	// NOT reload input on switch — only the history-nav cursor resets, below).
	// svelte-ignore state_referenced_locally
	let input = $state(drafts.get(composerKey(session.id)));
	$effect(() => {
		drafts.set(composerKey(session.id), input);
	});

	const stagedQuery = useSessionAttachments(
		() => session.id,
		() => supportsAttachments && !archived
	);
	const att = new ComposerAttachments({
		draftKey: () => composerKey(session.id),
		enabled: () => supportsAttachments && !archived,
		input: () => input,
		setInput: (text) => (input = text),
		stagedNames: () => (stagedQuery.data ?? []).map((a) => a.name)
	});
	onDestroy(() => att.images.reset());
	export function addFiles(incoming: File[]) {
		att.add(incoming);
	}
	export function setDragActive(active: boolean) {
		att.dragActive = active;
	}

	const cache = new CacheColdClock({
		adapterId: () => session.adapter_id,
		model: () => session.model ?? null,
		lastActivityAt: () => session.last_activity_at,
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
		!!input.trim() && att.files.length === 0 && !att.uploading && att.images.pending.length === 0
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
		if (!text || archived || att.files.length) return;
		try {
			await scheduledActions.schedule(text, at);
		} catch (e) {
			toasts.error(m.composer_schedule_failed({ message: errMessage(e) }));
			return;
		}
		toasts.info(
			m.composer_schedule_toast({
				when: at.toLocaleString([], {
					weekday: 'short',
					hour: '2-digit',
					minute: '2-digit'
				})
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

<div class="composer" data-journey="composer" class:dropping={att.dragActive}>
	{#if archived}
		<ArchivedActions {onNewFromScript} {onFork} {onResume} />
	{:else}
		<!-- Failed sends surface inline on the message bubble itself (red +
		     Retry), so there's no separate composer banner. -->
		<div class="scheduled" bind:this={scheduledEl}><ScheduledMessages sessionId={session.id} {archived} /></div>
		<ImageCompressionStatus pending={att.images.pending} />
		{#if supportsAttachments && att.files.length}
			<div class="attachments">
				<AttachmentList files={att.files} onremove={(name) => att.remove(name)} compact />
			</div>
		{/if}
		{#if coldOffer}
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
		<InputGroup leading={supportsAttachments ? attach : undefined}>
			<SessionMention
				bind:value={input}
				el={scroll.textarea}
				sessions={mentionSessions}
				excludeId={session.id}
				placement="up"
			>
				<Textarea
					rows={1}
					autoresize
					resize="top"
					maxHeight="40vh"
					submitOn={coarsePointer ? 'mod-enter' : 'enter'}
					onsubmit={submit}
					data-journey="message"
					aria-label={m.a11y_composer_message()}
					placeholder={att.dragActive
						? m.composer_drop_files()
						: coarsePointer
							? m.composer_placeholder_message()
							: m.composer_placeholder_message_enter()}
					bind:value={input}
					bind:el={scroll.textarea}
					onkeydown={onKey}
					oninput={() => resetHistoryNav()}
					onpaste={(e) => att.onPaste(e)}
				/>
			</SessionMention>
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
		padding-bottom: var(--safe-bottom);
		border-top: 1px solid var(--border);
		background: var(--bg-elevated);
	}
	/* The field runs edge to edge; the rows above it keep their own inset. */
	.scheduled,
	.cold-offer,
	.attachments {
		padding: var(--sp-2) var(--sp-3);
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
