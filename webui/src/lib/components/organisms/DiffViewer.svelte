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
	import { untrack } from 'svelte';
	import {
		flattenDiff,
		navIndex,
		weaveComments,
		indexComments,
		indexThreads,
		lineAnchor,
		type DiffRow,
		type DiffViewMode,
		type CommentAnchorKey
	} from '$lib/diff/rows';
	import { langForPath } from '$lib/diff/highlight';
	import type { BlockSelection } from '$lib/diff/ask';
	import DiffRowView from '../molecules/DiffRow.svelte';
	import { Badge, Button, Cluster, Select, Stack, Text, Textarea } from '@dorsk/tsumikit';
	import {
		endpoints,
		useGithubDrafts,
		githubDraftsKey,
		useGithubThreads,
		githubThreadsKey,
		useGithubViewed,
		githubViewedKey
	} from '$lib/queries';
	import { useQueryClient } from '@tanstack/svelte-query';
	import type { ReviewVerdict } from '@bindings/ReviewVerdict';
	import type { PublishReviewResult } from '@bindings/PublishReviewResult';

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
		/** GH-AGENT-3: "Ask the agent about this block". When provided, the diff
		 *  viewer surfaces an action (button per line + the `a` key) that hands the
		 *  reviewer's selected block — path, side, line, and the snippet TEXT — to
		 *  the host, which injects it into the linked review session. No checkout:
		 *  the agent gets the snippet inline (docs §6.3). */
		onask?: (sel: BlockSelection) => void;
	}
	const {
		diff,
		connectorId,
		number,
		lens = 'cumulative',
		commits = [],
		onlens,
		onask
	}: Props = $props();
	const askable = $derived(!!onask);

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

	// Pulled-down existing GitHub review threads (GH-VIEW-5), rendered inline and
	// visually distinct from local drafts.
	const threadsQuery = useGithubThreads(
		() => connectorId ?? '',
		() => diff.repo,
		() => number ?? 0,
		() => commentable
	);
	const threads = $derived($threadsQuery.data ?? []);
	const threadIndex = $derived(indexThreads(threads));

	const qc = useQueryClient();
	function refreshDrafts() {
		if (connectorId && number != null)
			qc.invalidateQueries({ queryKey: githubDraftsKey(connectorId, diff.repo, number) });
	}
	function refreshThreads() {
		if (connectorId && number != null)
			qc.invalidateQueries({ queryKey: githubThreadsKey(connectorId, diff.repo, number) });
	}

	// ---- GH-VIEW-6: blob-keyed "reviewed" marks --------------------------------
	// Mark a file reviewed keyed to its CURRENT blob SHA. On reload after a push,
	// a file stays reviewed only while its current blob SHA still equals the
	// stored mark — so a push re-flags ONLY the files that actually changed; the
	// unchanged ones stay reviewed. The re-flag is this pure comparison, done at
	// render time against the live diff, not a server-side rewrite of the marks.
	const viewedQuery = useGithubViewed(
		() => connectorId ?? '',
		() => diff.repo,
		() => number ?? 0,
		() => commentable
	);
	// path -> blob_sha that was marked reviewed.
	const markedSha = $derived(
		new Map(($viewedQuery.data ?? []).map((m) => [m.path, m.blob_sha] as const))
	);
	// The set of file paths that are CURRENTLY reviewed: a mark exists AND the
	// file's current blob SHA still matches it (else the file changed → re-flag).
	const reviewedPaths = $derived(
		new Set(
			diff.files
				.filter((f) => f.blob_sha != null && markedSha.get(f.path) === f.blob_sha)
				.map((f) => f.path)
		)
	);
	const reviewedCount = $derived(reviewedPaths.size);

	function refreshViewed() {
		if (connectorId && number != null)
			qc.invalidateQueries({ queryKey: githubViewedKey(connectorId, diff.repo, number) });
	}

	async function toggleReviewed(path: string) {
		if (!connectorId || number == null) return;
		const file = diff.files.find((f) => f.path === path);
		if (!file?.blob_sha) return; // can't blob-key a file with no head blob SHA
		busy = true;
		try {
			if (reviewedPaths.has(path)) {
				await endpoints.unmarkGithubViewed(connectorId, diff.repo, number, {
					path,
					blob_sha: null
				});
				// Un-reviewing a file unfolds it again.
				const next = new Set(collapsedFiles);
				next.delete(path);
				collapsedFiles = next;
			} else {
				await endpoints.markGithubViewed(connectorId, diff.repo, number, {
					path,
					blob_sha: file.blob_sha
				});
				// Reviewed files collapse to keep the surface focused on what's left.
				const next = new Set(collapsedFiles);
				next.add(path);
				collapsedFiles = next;
			}
			refreshViewed();
		} finally {
			busy = false;
		}
	}

	// ---- GH-VIEW-5: publish the open draft as one batched GitHub review --------
	let verdict = $state<ReviewVerdict>('comment');
	let summary = $state('');
	let publishing = $state(false);
	let publishResult = $state<PublishReviewResult | null>(null);
	let publishError = $state<string | null>(null);
	// Keep the picker in sync with the open draft's stored verdict.
	$effect(() => {
		if (openDraft) verdict = openDraft.verdict;
	});
	const canPublish = $derived(
		commentable && !!openDraft && (openDraft.comments.length > 0 || summary.trim().length > 0)
	);

	async function publishReview() {
		if (!connectorId || number == null || !openDraft) return;
		publishing = true;
		publishError = null;
		publishResult = null;
		try {
			// Sync the verdict first if the reviewer changed it in the picker.
			if (verdict !== openDraft.verdict)
				await endpoints.updateGithubDraft(connectorId, diff.repo, number, openDraft.id, { verdict });
			publishResult = await endpoints.publishGithubReview(connectorId, diff.repo, number, {
				draft_id: openDraft.id,
				summary: summary.trim() ? summary.trim() : null,
				// The head SHA the reviewer was viewing — the server refuses if the PR
				// has rotated past it (force-push) rather than mis-placing comments.
				expected_head_sha: diff.head_sha
			});
			summary = '';
			refreshDrafts();
			refreshThreads();
		} catch (e) {
			// The server returns a clear message for stale-SHA / empty-review / anchor
			// failures; surface it verbatim (it carries no secrets).
			publishError = e instanceof Error ? e.message : 'Failed to publish review';
		} finally {
			publishing = false;
		}
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

	// GH-AGENT-3: hand the diff line under `row` to the host as a block selection
	// (path + side + line + the line's text), so it can "Ask the agent about this
	// block". Single-line for now (the viewer's selection unit is one line).
	function askBlock(row: DiffRow) {
		if (!onask || row.kind !== 'line') return;
		const a = lineAnchor(row.line);
		if (!a) return;
		onask({
			path: row.fileKey,
			side: a.side,
			line: a.line,
			startLine: null,
			snippet: row.line.content
		});
	}

	// Resolve the cursor row to a commentable/askable line, working for both the
	// unified (`line`) and split (`pair`) layouts. For a pair prefer the new side
	// (right), falling back to the old side (left) — matching where a single
	// keystroke most usefully lands.
	function cursorLine(): {
		fileKey: string;
		anchor: { side: DiffSide; line: number };
		snippet: string;
	} | null {
		const r = rows[cursor];
		if (!r) return null;
		if (r.kind === 'line') {
			const a = lineAnchor(r.line);
			return a ? { fileKey: r.fileKey, anchor: a, snippet: r.line.content } : null;
		}
		if (r.kind === 'pair') {
			const cell = r.right ?? r.left;
			if (!cell) return null;
			const a = lineAnchor(cell);
			return a ? { fileKey: r.fileKey, anchor: a, snippet: cell.content } : null;
		}
		return null;
	}

	// Lazy-expanded collapsed regions + folded files — caller-free UI state.
	let expanded = $state(new Set<string>());
	let collapsedFiles = $state(new Set<string>());
	// Unified (vertical) vs side-by-side (split) layout — reviewer's choice,
	// persisted across PRs so the preference sticks for the session.
	let viewMode = $state<DiffViewMode>(loadViewMode());
	function loadViewMode(): DiffViewMode {
		if (typeof localStorage === 'undefined') return 'unified';
		return localStorage.getItem('cctui.diff.viewMode') === 'split' ? 'split' : 'unified';
	}
	function setViewMode(m: DiffViewMode) {
		viewMode = m;
		if (typeof localStorage !== 'undefined') localStorage.setItem('cctui.diff.viewMode', m);
	}

	const baseRows = $derived<DiffRow[]>(flattenDiff(diff, expanded, collapsedFiles, viewMode));
	const rows = $derived<DiffRow[]>(
		weaveComments(baseRows, commentIndex, composeAt, threadIndex)
	);
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

	// Create the virtualizer ONCE. It must not be re-created reactively: the
	// `getScrollElement` closure is not a tracked dependency, so re-creating it
	// (the old `$derived(createVirtualizer(...))`) only happened when `rows.length`
	// changed — and for a PR with no drafts/threads that never changes after
	// mount. The single instance was then built at first render while `scrollEl`
	// (bound via `bind:this`) was still undefined, so it captured a null scroll
	// element, never measured the viewport, and rendered a scrollbar (height from
	// the estimate) with zero on-screen rows. We instead re-apply options in an
	// `$effect` that runs AFTER mount, once the scroll element exists.
	const virtualizer = createVirtualizer<HTMLDivElement, HTMLDivElement>({
		count: rows.length,
		getScrollElement: () => scrollEl ?? null,
		estimateSize: () => 21,
		overscan: 20
	});
	$effect(() => {
		// Track row count + scroll element so a settled query (new rows) or the
		// post-mount `bind:this` assignment re-applies options → the virtualizer
		// picks up the now-mounted scroll element and measures the viewport.
		const count = rows.length;
		const el = scrollEl;
		// CRITICAL: `untrack` the store read/write. `$virtualizer.setOptions` both
		// READS the virtualizer store (subscribing this effect to it) and triggers
		// its `onChange` (a store `set`). Without untrack, every row measurement
		// (`use:measure` → `measureElement`, whose real heights differ from the
		// 21px estimate → `onChange`) updates the store → re-runs this effect →
		// setOptions → onChange → … → `effect_update_depth_exceeded`. That loop
		// crashed the viewer right after the first viewport (~50 rows). We only
		// want this effect to re-run on `count`/`el` changes (read above, tracked).
		untrack(() => {
			$virtualizer.setOptions({
				count,
				getScrollElement: () => el ?? null,
				estimateSize: () => 21,
				overscan: 20
			});
		});
	});

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
			case 'v': {
				// GH-VIEW-6: toggle the reviewed mark on the file under the cursor.
				if (!commentable) return;
				const r = rows[cursor];
				if (!r) return;
				void toggleReviewed(r.fileKey);
				break;
			}
			case 'c': {
				// GH-VIEW-4: open the inline composer on the cursor's diff line.
				if (!commentable) return;
				const t = cursorLine();
				if (!t) return;
				startCompose(t.fileKey, t.anchor.side, t.anchor.line);
				break;
			}
			case 'a': {
				// GH-AGENT-3: ask the linked review agent about the cursor's line.
				if (!askable) return;
				const t = cursorLine();
				if (!t) return;
				onask?.({
					path: t.fileKey,
					side: t.anchor.side,
					line: t.anchor.line,
					startLine: null,
					snippet: t.snippet
				});
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
			{#if commentable}
				<Badge tone={reviewedCount === diff.files.length ? 'ok' : 'neutral'}>
					{reviewedCount} of {diff.files.length} files reviewed
				</Badge>
			{/if}
			{#if diff.huge}
				<Badge tone="warn">huge diff — showing {diff.files.length} of {diff.total_files} files</Badge
				>
			{/if}
		</Cluster>
		<Cluster gap="var(--sp-3)" align="center">
			<Cluster gap="var(--sp-1)" align="center">
				<Text tone="muted" size="xs">View:</Text>
				<button
					type="button"
					class="lens"
					class:on={viewMode === 'unified'}
					title="Unified (vertical) diff"
					onclick={() => setViewMode('unified')}>Unified</button
				>
				<button
					type="button"
					class="lens"
					class:on={viewMode === 'split'}
					title="Side-by-side (split) diff"
					onclick={() => setViewMode('split')}>Split</button
				>
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
	</Cluster>

	{#if diff.huge}
		<Text tone="muted" size="xs">
			This PR exceeds the large-diff threshold; GitHub serves it unreliably, so only the first
			{diff.files.length} files are loaded. Open individual files on GitHub for the rest.
		</Text>
	{/if}

	{#if commentable}
		<!-- GH-VIEW-5: publish the open draft as ONE batched GitHub review. -->
		<div class="publish">
			<Cluster gap="var(--sp-2)" align="center" justify="space-between">
				<Cluster gap="var(--sp-2)" align="center">
					<Text size="sm" tone="muted">
						{openDraft ? `${openDraft.comments.length} draft comment(s)` : 'No open draft'}
					</Text>
					<Select bind:value={verdict} disabled={!openDraft || publishing}>
						<option value="comment">Comment</option>
						<option value="approve">Approve</option>
						<option value="request_changes">Request changes</option>
					</Select>
				</Cluster>
				<Button
					variant="primary"
					onclick={publishReview}
					disabled={!canPublish || publishing}
				>
					{publishing ? 'Publishing…' : 'Publish review'}
				</Button>
			</Cluster>
			<Textarea
				bind:value={summary}
				rows={2}
				placeholder="Optional review summary…"
				disabled={!openDraft || publishing}
			/>
			{#if publishError}
				<Text tone="danger" size="sm">{publishError}</Text>
			{:else if publishResult}
				<Text tone="success" size="sm">
					Published {publishResult.submitted} comment(s){publishResult.skipped.length
						? `, skipped ${publishResult.skipped.length} un-anchorable`
						: ''}.
				</Text>
				{#each publishResult.skipped as s (s.comment_id)}
					<Text tone="warn" size="xs">
						skipped {s.path}:{s.line} — {s.reason.kind === 'stale_head_sha'
							? 'PR moved (force-push)'
							: s.reason.kind === 'file_not_found'
								? 'file no longer in diff'
								: s.reason.kind === 'line_not_in_diff'
									? 'line no longer in diff'
									: 'invalid range'}
					</Text>
				{/each}
			{/if}
		</div>
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
				{#if rows[vrow.index]}
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
						reviewed={reviewedPaths.has(rows[vrow.index].fileKey)}
						ontoggleReviewed={commentable ? toggleReviewed : undefined}
						onexpand={expand}
						ontoggleFile={toggleFile}
						{commentable}
						{busy}
						oncommentLine={startCompose}
						onsaveComment={saveComment}
						oneditComment={editComment}
						ondeleteComment={deleteComment}
						oncancelComment={cancelCompose}
						onaskLine={askable
							? (fileKey, side, line, snippet) =>
									onask?.({ path: fileKey, side, line, startLine: null, snippet })
							: undefined}
					/>
				</div>
				{/if}
			{/each}
		</div>
	</div>
	<Text tone="muted" size="xs">
		j/k line · n/p hunk · ]/[ file · o expand/fold{commentable
			? ' · c comment · v reviewed'
			: ''}{askable ? ' · a ask agent' : ''}
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
	.publish {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-2);
		border: 1px solid var(--border, rgba(127, 127, 127, 0.25));
		border-radius: var(--radius, 6px);
		background: var(--surface-1, rgba(127, 127, 127, 0.04));
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
