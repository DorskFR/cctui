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
	import { createVirtualizer } from '@tanstack/svelte-virtual';
	import { flattenDiff, navIndex, type DiffRow } from '$lib/diff/rows';
	import { langForPath } from '$lib/diff/highlight';
	import DiffRowView from '../molecules/DiffRow.svelte';
	import { Badge, Cluster, Stack, Text } from '@dorsk/tsumikit';

	interface Props {
		diff: PullDiff;
		/** Optional per-commit lens controls (GH-VIEW-3 lenses). `commits` is the
		 *  PR's commit list; selecting one re-fetches that commit's diff via the
		 *  proxy. When absent only the cumulative lens shows. */
		lens?: string; // 'cumulative' | a commit SHA
		commits?: { sha: string; subject: string }[];
		onlens?: (lens: string) => void;
	}
	const { diff, lens = 'cumulative', commits = [], onlens }: Props = $props();

	// Lazy-expanded collapsed regions + folded files — caller-free UI state.
	let expanded = $state(new Set<string>());
	let collapsedFiles = $state(new Set<string>());

	const rows = $derived<DiffRow[]>(flattenDiff(diff, expanded, collapsedFiles));
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
					/>
				</div>
			{/each}
		</div>
	</div>
	<Text tone="muted" size="xs">
		j/k line · n/p hunk · ]/[ file · o expand/fold
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
