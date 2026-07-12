<!--
  The block↔conversation bridge (GH-AGENT-3, docs §6.3) — the headline review
  UX. The virtualized diff viewer and the existing conversation drawer sit SIDE
  BY SIDE for one PR and its linked "Review with agent" session (spawned by
  GH-AGENT-1, linked via SessionChild{kind:"pr"} — here passed in as the
  `session` prop, since the inbox owns finding it).

  Two actions wire the two panes together:
   1. "Ask the agent about this block" — selecting a diff line/block emits a
      `BlockSelection`; we assemble a message (path + line range + the snippet
      TEXT, no checkout — docs §6.3) and inject it into the session via
      `ws.sendMessage`. We remember the block as the "pending" one.
   2. "Promote answer to draft comment" — the agent's latest answer (tracked off
      `ws.onStream`, the component-local reactivity pattern) can be promoted to a
      draft comment anchored to the pending block (VIEW-4 draft store).

  Human curates drafts; nothing reaches GitHub until Publish (VIEW-5, owned by
  the diff viewer's publish bar).
-->
<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import type { PullDiff } from '@bindings/PullDiff';
	import { ws } from '$lib/ws.svelte';
	import { endpoints, useGithubDrafts, githubDraftsKey } from '$lib/queries';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { userMsgKey } from '$lib/ws.svelte';
	import { blockAskMessage, blockLabel, promoteAnswerToDraft, type BlockSelection } from '$lib/diff/ask';
	import { toasts } from '$lib/toast.svelte';
	import { Button, Cluster, Stack, Text } from '@dorsk/tsumikit';
	import DiffViewer from './DiffViewer.svelte';
	import ConversationDrawer from './ConversationDrawer.svelte';
	import { m } from '$lib/paraglide/messages';

	interface Props {
		/** The PR's structured diff (GH-VIEW-1), already fetched by the host. */
		diff: PullDiff;
		/** PR locator for the draft store + diff commenting. */
		connectorId: string;
		number: number;
		/** The linked "Review with agent" session (GH-AGENT-1). */
		session: SessionListItem;
		onclose: () => void;
	}
	const { diff, connectorId, number, session, onclose }: Props = $props();

	const repo = $derived(diff.repo);
	const qc = useQueryClient();

	const draftsQuery = useGithubDrafts(
		() => connectorId,
		() => repo,
		() => number
	);

	// The block the reviewer last asked about — the promote action anchors the
	// agent's answer back onto it.
	let pending = $state<BlockSelection | null>(null);

	// Latest assistant answer text, tracked live off the stream (NOT a $derived
	// read of the ws singleton's keyed $state — see the webui reactivity memo).
	let lastAnswer = $state('');
	$effect(() => {
		const sid = session.id;
		// Seed from whatever is already buffered for this session.
		for (const ev of ws.bufferedEvents(sid)) {
			if (ev.type === 'text' && !ev.meta && userMsgKey(ev) === null) lastAnswer = ev.content;
		}
		const unsub = ws.onStream(sid, (ev) => {
			if (ev.type === 'text' && !ev.meta && userMsgKey(ev) === null) lastAnswer = ev.content;
		});
		return unsub;
	});

	// 1) Ask the agent about a block: assemble the message (snippet inline, no
	// checkout) and inject it into the session via the shared send path.
	function askAboutBlock(sel: BlockSelection) {
		pending = sel;
		const text = blockAskMessage(sel);
		ws.trackedSend(session.id, text, Date.now());
		toasts.push(m.review_asked_agent_about({ block: blockLabel(sel) }), 'info');
	}

	// 2) Promote the agent's latest answer to a draft comment anchored to the
	// pending block (opens a draft lazily, like the inline composer does).
	let promoting = $state(false);
	const canPromote = $derived(!!pending && lastAnswer.trim().length > 0 && !promoting);
	async function promoteAnswer() {
		if (!pending || !lastAnswer.trim()) return;
		promoting = true;
		try {
			const drafts = $draftsQuery.data ?? [];
			let draftId = drafts.find((d) => d.status === 'draft')?.id;
			if (!draftId) {
				const d = await endpoints.openGithubDraft(connectorId, repo, number, { verdict: null });
				draftId = d.id;
			}
			await endpoints.addGithubDraftComment(
				connectorId,
				repo,
				number,
				draftId,
				promoteAnswerToDraft(pending, lastAnswer)
			);
			qc.invalidateQueries({ queryKey: githubDraftsKey(connectorId, repo, number) });
			toasts.push(m.review_promoted_answer({ block: blockLabel(pending) }), 'ok');
		} catch (e) {
			toasts.push(e instanceof Error ? e.message : m.review_promote_failed(), 'err');
		} finally {
			promoting = false;
		}
	}
</script>

<div class="workspace">
	<div class="pane diff">
		<DiffViewer {diff} {connectorId} {number} onask={askAboutBlock} />
	</div>
	<div class="pane convo">
		<Stack gap="var(--sp-2)">
			<Cluster gap="var(--sp-2)" align="center" justify="space-between">
				<Text size="sm" tone="muted">
					{#if pending}
						{m.review_asked_about_label()} <code>{blockLabel(pending)}</code>
					{:else}
						{m.review_select_line_hint()}
					{/if}
				</Text>
				<Button onclick={promoteAnswer} disabled={!canPromote}>
					{promoting ? m.review_promoting() : m.review_promote_answer()}
				</Button>
			</Cluster>
			<div class="drawer-host">
				<ConversationDrawer {session} {onclose} />
			</div>
		</Stack>
	</div>
</div>

<style>
	.workspace {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: var(--sp-3);
		align-items: start;
	}
	.pane {
		min-width: 0;
	}
	.drawer-host {
		position: relative;
		min-height: 60vh;
	}
	@media (max-width: 900px) {
		.workspace {
			grid-template-columns: 1fr;
		}
	}
</style>
