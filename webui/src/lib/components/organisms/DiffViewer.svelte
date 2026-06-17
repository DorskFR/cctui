<!--
  The virtualized single-surface diff viewer (GH-VIEW-3, docs §6.2).

  ONE `@tanstack/svelte-virtual` instance renders the entire PR — every file's
  hunks and lines flattened into a single `DiffRow[]` (`flattenDiff`) — but only
  the on-screen slice is ever in the DOM. This is the DiffsHub model: it scales
  to thousands of files / 100k+ lines because cost is O(viewport), not O(diff).

  Behaviours:
   - importance ordering (source-before-support / by change size) via `orderFiles`
   - collapse-unchanged regions with lazy-expand (the `collapsed` marker rows)
   - per-file fold/unfold (click a file header)
   - keyboard nav: j/k line, n/p hunk, ]/[ file, o expand-region/toggle-file.
     Comment / toggle-reviewed / toggle-lens get leave-hooks later tickets fill.
   - lenses: cumulative diff (whole PR) + per-commit diff — both from GitHub via
     the proxy. (No working-copy lens — that needed a checkout, ruled out.)
   - large-diff affordance for the `huge` / per-file `truncated` cases (GH-VIEW-1).
-->
<script lang="ts">
	import type { PullDiff } from '@bindings/PullDiff';
	import type { DiffSide } from '@bindings/DiffSide';
	import { createVirtualizer } from '@tanstack/svelte-virtual';
	import {
		flattenDiff,
		navIndex,
		weaveComments,
		indexComments,
		lineAnchor,
		type DiffRow,
		type CommentAnchorKey
	} from '$lib/diff/rows';
	import { langForPath } from '$lib/diff/highlight';
	import DiffRowView from '../molecules/DiffRow.svelte';
	import { Badge, Cluster, Stack, Text } from '@dorsk/tsumikit';
	import { endpoints, useGithubDrafts, githubDraftsKey } from '$lib/queries';
	import { useQueryClient } from '@tanstack/svelte-query';

	interface Props {
		diff: PullDiff;
		/** PR locator for inline draft commenting (GH-VIEW-4). When omitted the
		 *  viewer renders read-only (e.g. an embedded preview). */
		connectorId?: string;
		number?: number;
		/** Optional per-commit lens controls (GH-VIEW-3 lenses). `commits` is the
		 *  PR's commit list; selecting one re-fetches that commit's diff via the
		 *  proxy. When absent only the cumulative lens shows. */
		lens?: string; // 'cumulative' | a commit SHA
		commits?: { sha: string; subject: string }[];
		onlens?: (lens: string) => void;
	}
	const { diff, connectorId, number, lens = 'cumulative', commits = [], onlens }: Props = $props();

	// Inline draft commenting is available when we have a PR locator (GH-VIEW-4).
	const commentable = $derived(!!connectorId && number != null);

	// The caller's review drafts for this PR (+ their inline comments). The first
	// draft is the "open" one we add comments to; opened lazily on first comment.
	const draftsQuery = useGithubDrafts(
		() => connectorId ?? '',
		() => diff.repo,
		() => number ?? 0,
		() => commentable
	);
	const drafts = $derived($draftsQuery.data ?? []);
	const openDraft = $derived(drafts.find((d) => d.status === 'draft'));
	const allComments = $derived(drafts.flatMap((d) => d.comments));
	const commentIndex = $derived(indexComments(allComments));
	// Map a comment id back to its owning draft, so edit/delete target the right
	// draft even when the PR has multiple (e.g. a human + an agent draft).
	const draftOfComment = $derived(
		new Map(drafts.flatMap((d) => d.comments.map((c) => [c.id, d.id] as const)))
	);

	const qc = useQueryClient();
	function refreshDrafts() {
		if (connectorId && number != null)
			qc.invalidateQueries({ queryKey: githubDraftsKey(connectorId, diff.repo, number) });
	}

	// Which line the reviewer is currently composing a comment on (GH-VIEW-4).
	let composeAt = $state<CommentAnchorKey | null>(null);
	let busy = $state(false);

	function startCompose(path: string, side: DiffSide, line: number) {
		composeAt = { path, side, line };
	}
	function cancelCompose() {
		composeAt = null;
	}

	/** Ensure an open draft exists, then add the composed comment to it. The
	 *  comment lands INSTANTLY — one POST, no GitHub round-trip (docs §6.2). */
	async function saveComment(body: string) {
		if (!connectorId || number == null || !composeAt) return;
		busy = true;
		try {
			let draftId = openDraft?.id;
			if (!draftId) {
				const d = await endpoints.openGithubDraft(connectorId, diff.repo, number, {
					verdict: null
				});
				draftId = d.id;
			}
			await endpoints.addGithubDraftComment(connectorId, diff.repo, number, draftId, {
				path: composeAt.path,
				side: composeAt.side,
				line: composeAt.line,
				start_line: null,
				body,
				in_reply_to: null
			});
			composeAt = null;
			refreshDrafts();
		} finally {
			busy = false;
		}
	}

	async function editComment(commentId: string, body: string) {
		const draftId = draftOfComment.get(commentId);
		if (!connectorId || number == null || !draftId) return;
		busy = true;
		try {
			await endpoints.updateGithubDraftComment(connectorId, diff.repo, number, draftId, commentId, {
				body
			});
			refreshDrafts();
		} finally {
			busy = false;
		}
	}

	async function deleteComment(commentId: string) {
		const draftId = draftOfComment.get(commentId);
		if (!connectorId || number == null || !draftId) return;
		busy = true;
		try {
			await endpoints.deleteGithubDraftComment(connectorId, diff.repo, number, draftId, commentId);
			refreshDrafts();
		} finally {
			busy = false;
		}
	}

	// Lazy-expanded collapsed regions + folded files — caller-free UI state.
	let expanded = $state(new Set<string>());
	let collapsedFiles = $state(new Set<string>());

	const baseRows = $derived<DiffRow[]>(flattenDiff(diff, expanded, collapsedFiles));
	const rows = $derived<DiffRow[]>(weaveComments(baseRows, commentIndex, composeAt));
	const nav = $derived(navIndex(rows));

	// Cache the resolved language per file path so each row doesn't re-resolve.
	const langCache = new Map<string, string | null>();
	function langFor(path: string): string | null {
		let l = langCache.get(path);
		if (l === undefined) {
			l = langForPath(path);
			langCache.set(path, l);
		}
		return l;
	}

	let scrollEl = $state<HTMLDivElement>();
	let cursor = $state(0); // current keyboard row index

	const virtualizer = $derived(
		createVirtualizer<HTMLDivElement, HTMLDivElement>({
			count: rows.length,
			getScrollElement: () => scrollEl ?? null,
			estimateSize: () => 21,
			overscan: 20
		})
	);

	function expand(regionId: string) {
		const next = new Set(expanded);
		next.add(regionId);
		expanded = next;
	}
	function toggleFile(fileKey: string) {
		const next = new Set(collapsedFiles);
		if (next.has(fileKey)) next.delete(fileKey);
		else next.add(fileKey);
		collapsedFiles = next;
	}

	function moveTo(index: number) {
		if (index < 0 || index >= rows.length) return;
		cursor = index;
		$virtualizer.scrollToIndex(index, { align: 'center' });
	}
	/** Next index in `list` strictly greater than the cursor (wraps to first). */
	function nextIn(list: number[]): number {
		const hit = list.find((i) => i > cursor);
		return hit ?? list[0] ?? cursor;
	}
	function prevIn(list: number[]): number {
		for (let i = list.length - 1; i >= 0; i--) if (list[i] < cursor) return list[i];
		return list[list.length - 1] ?? cursor;
	}

	function onkeydown(e: KeyboardEvent) {
		if (e.metaKey || e.ctrlKey || e.altKey) return;
		switch (e.key) {
			case 'j':
				moveTo(Math.min(cursor + 1, rows.length - 1));
				break;
			case 'k':
				moveTo(Math.max(cursor - 1, 0));
				break;
			case 'n':
				moveTo(nextIn(nav.hunks));
				break;
			case 'p':
				moveTo(prevIn(nav.hunks));
				break;
			case ']':
				moveTo(nextIn(nav.files));
				break;
			case '[':
				moveTo(prevIn(nav.files));
				break;
			case 'o': {
				// Expand the region / toggle the file under the cursor.
				const r = rows[cursor];
				if (r?.kind === 'collapsed') expand(r.regionId);
				else if (r?.kind === 'file') toggleFile(r.fileKey);
				else return;
				break;
			}
			case 'c': {
				// GH-VIEW-4: open the inline composer on the cursor's diff line.
				if (!commentable) return;
				const r = rows[cursor];
				if (r?.kind !== 'line') return;
				const a = lineAnchor(r.line);
				if (!a) return;
				startCompose(r.fileKey, a.side, a.line);
				break;
			}
			default:
				return;
		}
		e.preventDefault();
	}

	// Re-measure each mounted row's real height so the flat `estimateSize` guess
	// is corrected for tall rows (wrapped headers, multi-line notes) — only the
	// on-screen rows are ever measured, keeping cost O(viewport).
	function measure(el: HTMLDivElement) {
		$virtualizer.measureElement(el);
	}

	const LENSES = $derived([
		{ id: 'cumulative', label: 'Cumulative' },
		...commits.map((c) => ({ id: c.sha, label: c.sha.slice(0, 7) }))
	]);
</script>

<Stack gap="var(--sp-2)">
	<Cluster gap="var(--sp-3)" align="center" justify="space-between">
		<Cluster gap="var(--sp-2)" align="center">
			<Text tone="muted" size="sm"
				>{diff.total_files} files · {diff.total_changes} changed lines</Text
			>
			{#if diff.huge}
				<Badge tone="warn">huge diff — showing {diff.files.length} of {diff.total_files} files</Badge
				>
			{/if}
		</Cluster>
		{#if LENSES.length > 1}
			<Cluster gap="var(--sp-1)" align="center">
				<Text tone="muted" size="xs">Lens:</Text>
				{#each LENSES as l (l.id)}
					<button
						type="button"
						class="lens"
						class:on={lens === l.id}
						onclick={() => onlens?.(l.id)}>{l.label}</button
					>
				{/each}
			</Cluster>
		{/if}
	</Cluster>

	{#if diff.huge}
		<Text tone="muted" size="xs">
			This PR exceeds the large-diff threshold; GitHub serves it unreliably, so only the first
			{diff.files.length} files are loaded. Open individual files on GitHub for the rest.
		</Text>
	{/if}

	<!-- svelte-ignore a11y_no_noninteractive_tabindex a11y_no_noninteractive_element_interactions -->
	<div
		class="scroll"
		bind:this={scrollEl}
		tabindex="0"
		role="group"
		aria-label="Pull request diff"
		{onkeydown}
	>
		<div class="spacer" style="height: {$virtualizer.getTotalSize()}px;">
			{#each $virtualizer.getVirtualItems() as vrow (vrow.index)}
				<div
					class="vrow"
					style="transform: translateY({vrow.start}px);"
					data-index={vrow.index}
					use:measure
				>
					<DiffRowView
						row={rows[vrow.index]}
						lang={langFor(rows[vrow.index].fileKey)}
						active={vrow.index === cursor}
						fileCollapsed={collapsedFiles.has(rows[vrow.index].fileKey)}
						onexpand={expand}
						ontoggleFile={toggleFile}
						{commentable}
						{busy}
						oncommentLine={startCompose}
						onsaveComment={saveComment}
						oneditComment={editComment}
						ondeleteComment={deleteComment}
						oncancelComment={cancelCompose}
					/>
				</div>
			{/each}
		</div>
	</div>
	<Text tone="muted" size="xs">
		j/k line · n/p hunk · ]/[ file · o expand/fold{commentable ? ' · c comment' : ''}
	</Text>
</Stack>

<style>
	.scroll {
		height: 70vh;
		overflow: auto;
		border: 1px solid var(--border, rgba(127, 127, 127, 0.25));
		border-radius: var(--radius, 6px);
		outline: none;
	}
	.scroll:focus-visible {
		box-shadow: 0 0 0 2px var(--accent, #4c8bf5);
	}
	.spacer {
		position: relative;
		width: 100%;
	}
	.vrow {
		position: absolute;
		top: 0;
		left: 0;
		width: 100%;
	}
	.lens {
		border: 1px solid var(--border, rgba(127, 127, 127, 0.25));
		background: transparent;
		color: inherit;
		border-radius: var(--radius, 6px);
		padding: 1px var(--sp-2);
		cursor: pointer;
		font-size: 0.75rem;
		font-family: var(--font-mono, monospace);
	}
	.lens.on {
		background: var(--accent, #4c8bf5);
		color: #fff;
		border-color: var(--accent, #4c8bf5);
	}
</style>
