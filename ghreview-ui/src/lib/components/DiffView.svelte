<script lang="ts">
  import type { NavIndex } from "../diff/navindex";
  import type { DiffModel } from "../diff/parse";
  import { ROW_HEIGHT } from "../diff/canvas/layout";
  import { computeWindow } from "../diff/virtual";
  import { type LineAddress, type ReviewController, rowToAddress } from "../review/anchors";
  import InlineCommentComposer from "./InlineCommentComposer.svelte";
  import InlineThread from "./InlineThread.svelte";

  interface Props {
    model: DiffModel;
    nav: NavIndex;
    focusRow: number;
    onFocusRow: (rowIndex: number) => void;
    review?: ReviewController;
  }
  let { model, focusRow, onFocusRow, review }: Props = $props();

  const ROW_H = ROW_HEIGHT;
  let scrollTop = $state(0);
  let viewportH = $state(600);
  let container = $state<HTMLDivElement | null>(null);

  let pendingAddr = $state<{ addr: LineAddress; rowIndex: number } | null>(null);
  let openAnchor = $state<number | null>(null);

  const win = $derived(computeWindow(scrollTop, viewportH, ROW_H, model.rows.length, 40));
  const visible = $derived(model.rows.slice(win.start, win.end));

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
    const top = focusRow * ROW_H;
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
            <span class="code">{row.content}</span>
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
    </div>

    {#if review}
      <div class="threads">
        {#each review.anchors as anchor (anchor.rowIndex)}
          {#if openAnchor === anchor.rowIndex}
            <div class="panel" style:top="{(anchor.rowIndex + 1) * ROW_H}px">
              <InlineThread
                {anchor}
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
              style:top="{anchor.rowIndex * ROW_H}px"
              onclick={() => {
                openAnchor = anchor.rowIndex;
                pendingAddr = null;
              }}
            >{anchor.drafts.length + anchor.published.length}</button>
          {/if}
        {/each}

        {#if pendingAddr}
          <div class="panel" style:top="{(pendingAddr.rowIndex + 1) * ROW_H}px">
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
