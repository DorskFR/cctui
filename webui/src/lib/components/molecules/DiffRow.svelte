<!--
  One row of the virtualized diff surface (GH-VIEW-3). Renders exactly one
  `DiffRow` variant. Purely presentational: the parent `DiffViewer` owns the
  virtualizer, selection, and collapse/expand state; this only emits intents.

  Highlighting is per-line (`highlightLine`) so it costs nothing for the
  thousands of rows that aren't currently mounted by the virtualizer.
-->
<script lang="ts">
	import type { DiffRow } from '$lib/diff/rows';
	import { highlightLine } from '$lib/diff/highlight';
	import { Badge, Cluster, Text } from '@dorsk/tsumikit';

	interface Props {
		row: DiffRow;
		/** Resolved highlight.js language for this row's file, or null. */
		lang: string | null;
		/** Whether this row is the current keyboard cursor target. */
		active?: boolean;
		onexpand?: (regionId: string) => void;
		ontoggleFile?: (fileKey: string) => void;
		/** A file is folded (only its header shows). */
		fileCollapsed?: boolean;
	}
	const { row, lang, active = false, onexpand, ontoggleFile, fileCollapsed = false }: Props =
		$props();

	const statusTone: Record<string, 'ok' | 'danger' | 'warn' | 'neutral'> = {
		added: 'ok',
		removed: 'danger',
		modified: 'warn',
		renamed: 'neutral',
		copied: 'neutral',
		changed: 'warn'
	};
</script>

{#if row.kind === 'file'}
	<button
		type="button"
		class="file"
		class:active
		onclick={() => ontoggleFile?.(row.fileKey)}
		title={fileCollapsed ? 'Expand file' : 'Collapse file'}
	>
		<Cluster gap="var(--sp-2)" align="center" justify="space-between">
			<Cluster gap="var(--sp-2)" align="baseline">
				<span class="caret" class:folded={fileCollapsed}>▾</span>
				<Text weight="semibold" truncate>{row.file.path}</Text>
				{#if row.file.previous_path}
					<Text tone="muted" size="xs">← {row.file.previous_path}</Text>
				{/if}
			</Cluster>
			<Cluster gap="var(--sp-2)" align="center">
				<Badge tone={statusTone[row.file.status] ?? 'neutral'}>{row.file.status}</Badge>
				<Text tone="muted" size="xs"
					><span class="add">+{row.file.additions}</span>
					<span class="del">−{row.file.deletions}</span></Text
				>
			</Cluster>
		</Cluster>
	</button>
{:else if row.kind === 'hunk'}
	<div class="hunk" class:active>{row.header}</div>
{:else if row.kind === 'collapsed'}
	<button type="button" class="collapsed" onclick={() => onexpand?.(row.regionId)}>
		⋯ {row.count} unchanged {row.count === 1 ? 'line' : 'lines'} — click to expand
	</button>
{:else if row.kind === 'line'}
	<div class="line k-{row.line.kind}" class:active>
		<span class="num old">{row.line.old_line ?? ''}</span>
		<span class="num new">{row.line.new_line ?? ''}</span>
		<span class="marker"
			>{row.line.kind === 'add' ? '+' : row.line.kind === 'del' ? '−' : ' '}</span
		>
		<!-- eslint-disable-next-line svelte/no-at-html-tags -->
		<code class="content">{@html highlightLine(row.line.content, lang)}</code>
	</div>
{:else if row.kind === 'binary'}
	<div class="note"><Badge tone="neutral">binary</Badge> Binary file not shown</div>
{:else if row.kind === 'truncated'}
	<div class="note">
		<Badge tone="warn">large</Badge> This file's diff was too large to inline. Open it on GitHub to
		view the full change.
	</div>
{:else if row.kind === 'empty'}
	<div class="note"><Text tone="muted">No textual changes.</Text></div>
{/if}

<style>
	.file {
		width: 100%;
		text-align: left;
		background: var(--surface-2, rgba(127, 127, 127, 0.08));
		border: 0;
		border-top: 1px solid var(--border, rgba(127, 127, 127, 0.25));
		padding: var(--sp-2) var(--sp-3);
		cursor: pointer;
		font: inherit;
		color: inherit;
	}
	.file.active,
	.hunk.active,
	.line.active {
		outline: 2px solid var(--accent, #4c8bf5);
		outline-offset: -2px;
	}
	.caret {
		display: inline-block;
		transition: transform 0.1s;
		font-size: 0.8em;
	}
	.caret.folded {
		transform: rotate(-90deg);
	}
	.add {
		color: var(--syn-ok, #2ea043);
	}
	.del {
		color: var(--syn-danger, #f85149);
	}
	.hunk {
		padding: 2px var(--sp-3);
		color: var(--syn-meta, #8b949e);
		background: var(--surface-1, rgba(127, 127, 127, 0.04));
		font-family: var(--font-mono, monospace);
		font-size: 0.82rem;
		white-space: pre;
	}
	.collapsed {
		width: 100%;
		text-align: left;
		border: 0;
		background: var(--surface-1, rgba(127, 127, 127, 0.04));
		color: var(--syn-meta, #8b949e);
		padding: 2px var(--sp-3);
		cursor: pointer;
		font: inherit;
		font-size: 0.8rem;
	}
	.collapsed:hover {
		background: var(--surface-2, rgba(127, 127, 127, 0.1));
	}
	.line {
		display: grid;
		grid-template-columns: 3.5em 3.5em 1em 1fr;
		font-family: var(--font-mono, monospace);
		font-size: 0.82rem;
		line-height: 1.5;
		white-space: pre;
	}
	.line.k-add {
		background: var(--diff-add-bg, rgba(46, 160, 67, 0.15));
	}
	.line.k-del {
		background: var(--diff-del-bg, rgba(248, 81, 73, 0.15));
	}
	.num {
		color: var(--syn-meta, #8b949e);
		text-align: right;
		padding-right: var(--sp-2);
		user-select: none;
		opacity: 0.7;
	}
	.marker {
		text-align: center;
		user-select: none;
	}
	.content {
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.note {
		padding: var(--sp-2) var(--sp-3);
		display: flex;
		gap: var(--sp-2);
		align-items: center;
	}
</style>
