<script lang="ts">
  import type { NavIndex } from "../diff/navindex";
  import type { DiffModel } from "../diff/parse";
  import { buildSplitModel } from "../diff/split";
  import { highlightLine, langForPath } from "../diff/highlight";
  import { computeWindow } from "../diff/virtual";

  const ROW_HEIGHT = 20;
  import { type LineAddress, type ReviewController, rowToAddress } from "../review/anchors";
  import InlineCommentComposer from "./InlineCommentComposer.svelte";
  import InlineThread from "./InlineThread.svelte";

  interface Props {
    model: DiffModel;
    nav: NavIndex;
    focusRow: number;
    onFocusRow: (rowIndex: number) => void;
    review?: ReviewController;
    mode?: "unified" | "split";
    owner?: string;
    repo?: string;
    account?: string;
  }
  let { model, focusRow, onFocusRow, review, mode = "unified", owner, repo, account }: Props =
    $props();

  const ROW_H = ROW_HEIGHT;
  let scrollTop = $state(0);
  let viewportH = $state(600);
  let container = $state<HTMLDivElement | null>(null);

  let pendingAddr = $state<{ addr: LineAddress; rowIndex: number } | null>(null);
  let openAnchor = $state<number | null>(null);

  const langByFile = $derived(model.files.map((f) => langForPath(f.filename)));
  function hl(content: string, fileIndex: number): string {
    return highlightLine(content, langByFile[fileIndex] ?? null);
  }

  const split = $derived(mode === "split" ? buildSplitModel(model) : null);
  const rowCount = $derived(split ? split.rows.length : model.rows.length);
  const win = $derived(computeWindow(scrollTop, viewportH, ROW_H, rowCount, 40));
  const visible = $derived(model.rows.slice(win.start, win.end));
  const visibleSplit = $derived(split ? split.rows.slice(win.start, win.end) : []);
  function displayRow(unifiedIndex: number): number {
    return split ? (split.unifiedToSplit.get(unifiedIndex) ?? unifiedIndex) : unifiedIndex;
  }

  function openComposer(rowIndex: number): void {
    if (!review) return;
    const addr = rowToAddress(model, rowIndex);
    if (!addr) return;
    pendingAddr = { addr, rowIndex };
    openAnchor = null;
  }

  function submitNew(body: string): void {
    if (!pendingAddr) return;
    review?.addComment(pendingAddr.addr, body);
    pendingAddr = null;
  }

  $effect(() => {
    const el = container;
    if (!el) return;
    const top = displayRow(focusRow) * ROW_H;
    const bottom = top + ROW_H;
    if (top < el.scrollTop || bottom > el.scrollTop + el.clientHeight) {
      el.scrollTop = Math.max(0, top - el.clientHeight / 3);
    }
  });
</script>

<div
  class="scroller"
  bind:this={container}
  bind:clientHeight={viewportH}
  onscroll={(e) => (scrollTop = e.currentTarget.scrollTop)}
>
  <div class="spacer" style:height="{win.totalHeight}px">
    <div class="rows" style:transform="translateY({win.offsetY}px)">
      {#if split}
        {#each visibleSplit as srow, i (win.start + i)}
          {@const focused =
            srow.kind === "pair"
              ? srow.left?.rowIndex === focusRow || srow.right?.rowIndex === focusRow
              : srow.rowIndex === focusRow}
          <div class="row row-{srow.kind === "pair" ? "pair" : srow.row.kind}" class:focus={focused} style:height="{ROW_H}px" role="row" tabindex="-1">
            {#if srow.kind === "file"}
              <span class="filehdr" class:collapsed={srow.row.collapsed}>{srow.row.content}</span>
            {:else if srow.kind === "hunk"}
              <span class="gutter"></span>
              <span class="hunkhdr">{srow.row.content}</span>
            {:else}
              {@const l = srow.left}
              {@const r = srow.right}
              <button
                type="button"
                class="side side-{l ? l.row.kind : "empty"}"
                tabindex="-1"
                onclick={() => l && onFocusRow(l.rowIndex)}
              >
                <span class="gutter">{l?.row.oldLine ?? ""}</span>
                <span class="marker">{l?.row.kind === "del" ? "−" : ""}</span>
                <span class="code code-hl">{@html l ? hl(l.row.content, l.row.fileIndex) : ""}</span>
                {#if review && l && l.row.kind !== "context"}
                  <span
                    class="add-comment"
                    role="button"
                    tabindex="-1"
                    aria-label="Comment on this line"
                    onclick={(e) => {
                      e.stopPropagation();
                      openComposer(l.rowIndex);
                    }}
                    onkeydown={() => {}}
                  >+</span>
                {/if}
              </button>
              <button
                type="button"
                class="side side-{r ? r.row.kind : "empty"}"
                tabindex="-1"
                onclick={() => r && onFocusRow(r.rowIndex)}
              >
                <span class="gutter">{r?.row.newLine ?? ""}</span>
                <span class="marker">{r?.row.kind === "add" ? "+" : ""}</span>
                <span class="code code-hl">{@html r ? hl(r.row.content, r.row.fileIndex) : ""}</span>
                {#if review && r && r.row.kind !== "context"}
                  <span
                    class="add-comment"
                    role="button"
                    tabindex="-1"
                    aria-label="Comment on this line"
                    onclick={(e) => {
                      e.stopPropagation();
                      openComposer(r.rowIndex);
                    }}
                    onkeydown={() => {}}
                  >+</span>
                {/if}
              </button>
            {/if}
          </div>
        {/each}
      {:else}
        {#each visible as row, i (win.start + i)}
          {@const idx = win.start + i}
          <div
            class="row row-{row.kind}"
            class:focus={idx === focusRow}
            style:height="{ROW_H}px"
            role="row"
            tabindex="-1"
            onclick={() => onFocusRow(idx)}
            onkeydown={() => {}}
          >
            {#if row.kind === "file"}
              <span class="filehdr" class:collapsed={row.collapsed}>{row.content}</span>
            {:else if row.kind === "hunk"}
              <span class="gutter"></span>
              <span class="hunkhdr">{row.content}</span>
            {:else}
              <span class="gutter">{row.oldLine ?? ""}</span>
              <span class="gutter">{row.newLine ?? ""}</span>
              <span class="marker">{row.kind === "add" ? "+" : row.kind === "del" ? "−" : ""}</span>
              <span class="code code-hl">{@html hl(row.content, row.fileIndex)}</span>
              {#if review}
                <button
                  type="button"
                  class="add-comment"
                  aria-label="Comment on this line"
                  onclick={(e) => {
                    e.stopPropagation();
                    openComposer(idx);
                  }}
                >+</button>
              {/if}
            {/if}
          </div>
        {/each}
      {/if}
    </div>

    {#if review}
      <div class="threads">
        {#each review.anchors as anchor (anchor.rowIndex)}
          {#if openAnchor === anchor.rowIndex}
            <div class="panel" style:top="{(displayRow(anchor.rowIndex) + 1) * ROW_H}px">
              <InlineThread
                {anchor}
                {owner}
                {repo}
                {account}
                pending={review.pending}
                onAdd={(body) => review?.addComment({ path: anchor.path, side: anchor.side, line: anchor.line }, body)}
                onEdit={(id, body) => review?.editComment(id, body)}
                onDelete={(id) => review?.deleteComment(id)}
                onClose={() => (openAnchor = null)}
              />
            </div>
          {:else}
            <button
              type="button"
              class="thread-badge"
              style:top="{displayRow(anchor.rowIndex) * ROW_H}px"
              onclick={() => {
                openAnchor = anchor.rowIndex;
                pendingAddr = null;
              }}
            >{anchor.drafts.length + anchor.published.length}</button>
          {/if}
        {/each}

        {#if pendingAddr}
          <div class="panel" style:top="{(displayRow(pendingAddr.rowIndex) + 1) * ROW_H}px">
            <InlineCommentComposer
              pending={review.pending}
              onsubmit={submitNew}
              oncancel={() => (pendingAddr = null)}
            />
          </div>
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .scroller {
    height: 100%;
    overflow: auto;
    font-family: var(--gh-mono);
    font-size: 12px;
    background: var(--gh-bg);
  }
  .spacer {
    position: relative;
  }
  .rows {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
  }
  .row {
    display: flex;
    align-items: center;
    white-space: pre;
    contain: layout paint;
    line-height: 20px;
    position: relative;
  }
  .row.focus {
    box-shadow: inset 2px 0 0 var(--gh-accent);
    background: color-mix(in srgb, var(--gh-accent) 8%, transparent);
  }
  .row-pair {
    padding: 0;
  }
  .side {
    flex: 1 1 0;
    min-width: 0;
    display: flex;
    align-items: center;
    height: 100%;
    background: transparent;
    border: none;
    border-right: 1px solid var(--gh-border-muted);
    color: inherit;
    font: inherit;
    text-align: left;
    padding: 0;
    cursor: pointer;
    position: relative;
    white-space: pre;
  }
  .side:last-child {
    border-right: none;
  }
  .side-add {
    background: var(--gh-diff-add-bg);
    color: var(--gh-diff-add-fg);
    box-shadow: inset 3px 0 0 var(--gh-diff-add-edge);
  }
  .side-add .marker {
    color: var(--gh-diff-add-glyph);
  }
  .side-del {
    background: var(--gh-diff-del-bg);
    color: var(--gh-diff-del-fg);
    box-shadow: inset 3px 0 0 var(--gh-diff-del-edge);
  }
  .side-del .marker {
    color: var(--gh-diff-del-glyph);
  }
  .side-empty {
    background: var(--gh-bg-inset);
    cursor: default;
  }
  .side .add-comment {
    left: 60px;
  }
  .side:hover .add-comment {
    opacity: 1;
  }
  .add-comment {
    position: absolute;
    left: 96px;
    width: 16px;
    height: 16px;
    line-height: 14px;
    padding: 0;
    font-size: 13px;
    color: white;
    background: var(--gh-accent);
    border: none;
    border-radius: var(--gh-radius-sm);
    cursor: pointer;
    opacity: 0;
  }
  .row:hover .add-comment {
    opacity: 1;
  }
  .threads {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    pointer-events: none;
  }
  .panel {
    position: absolute;
    left: 110px;
    right: 12px;
    pointer-events: auto;
  }
  .thread-badge {
    position: absolute;
    left: 84px;
    height: 16px;
    min-width: 18px;
    padding: 0 4px;
    font-size: 10px;
    line-height: 14px;
    color: white;
    background: var(--gh-accent);
    border: none;
    border-radius: 999px;
    cursor: pointer;
    pointer-events: auto;
  }
  .gutter {
    flex: none;
    width: 48px;
    text-align: right;
    padding-right: var(--gh-space-2);
    color: var(--gh-diff-gutter-fg);
    background: var(--gh-diff-gutter-bg);
    user-select: none;
  }
  .marker {
    flex: none;
    width: 14px;
    text-align: center;
    font-weight: 700;
    color: var(--gh-fg-subtle);
  }
  .code {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .row-add {
    background: var(--gh-diff-add-bg);
    color: var(--gh-diff-add-fg);
    box-shadow: inset 3px 0 0 var(--gh-diff-add-edge);
  }
  .row-add .gutter {
    background: var(--gh-diff-add-bg);
  }
  .row-add .marker {
    color: var(--gh-diff-add-glyph);
  }
  .row-del {
    background: var(--gh-diff-del-bg);
    color: var(--gh-diff-del-fg);
    box-shadow: inset 3px 0 0 var(--gh-diff-del-edge);
  }
  .row-del .gutter {
    background: var(--gh-diff-del-bg);
  }
  .row-del .marker {
    color: var(--gh-diff-del-glyph);
  }
  .row-file {
    background: var(--gh-bg-inset);
    border-top: 1px solid var(--gh-border);
    font-family: var(--gh-font);
    font-weight: 600;
    padding-left: var(--gh-space-3);
  }
  .row-hunk {
    color: var(--gh-diff-hunk-fg);
    background: var(--gh-diff-hunk-bg);
  }
  .filehdr {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .filehdr.collapsed {
    color: var(--gh-fg-muted);
    font-weight: 400;
    font-style: italic;
  }
  .hunkhdr {
    padding-left: var(--gh-space-2);
  }
</style>
