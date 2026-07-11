<script lang="ts">
  import type { NavIndex } from "../diff/navindex";
  import type { DiffModel } from "../diff/parse";
  import { computeWindow } from "../diff/virtual";

  interface Props {
    model: DiffModel;
    nav: NavIndex;
    focusRow: number;
    onFocusRow: (rowIndex: number) => void;
  }
  let { model, focusRow, onFocusRow }: Props = $props();

  const ROW_H = 20;
  let scrollTop = $state(0);
  let viewportH = $state(600);
  let container = $state<HTMLDivElement | null>(null);

  const win = $derived(computeWindow(scrollTop, viewportH, ROW_H, model.rows.length, 40));
  const visible = $derived(model.rows.slice(win.start, win.end));

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
            <span class="filehdr">{row.content}</span>
          {:else if row.kind === "hunk"}
            <span class="gutter"></span>
            <span class="hunkhdr">{row.content}</span>
          {:else}
            <span class="gutter">{row.oldLine ?? ""}</span>
            <span class="gutter">{row.newLine ?? ""}</span>
            <span class="marker">{row.kind === "add" ? "+" : row.kind === "del" ? "−" : ""}</span>
            <span class="code">{row.content}</span>
          {/if}
        </div>
      {/each}
    </div>
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
  }
  .row.focus {
    box-shadow: inset 2px 0 0 var(--gh-accent);
    background: color-mix(in srgb, var(--gh-accent) 8%, transparent);
  }
  .gutter {
    flex: none;
    width: 48px;
    text-align: right;
    padding-right: var(--gh-space-2);
    color: var(--gh-diff-gutter);
    user-select: none;
  }
  .marker {
    flex: none;
    width: 14px;
    text-align: center;
    color: var(--gh-fg-muted);
  }
  .code {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .row-add {
    background: var(--gh-diff-add-line);
  }
  .row-del {
    background: var(--gh-diff-del-line);
  }
  .row-file {
    background: var(--gh-bg-inset);
    border-top: 1px solid var(--gh-border);
    font-family: var(--gh-font);
    font-weight: 600;
    padding-left: var(--gh-space-3);
  }
  .row-hunk {
    color: var(--gh-accent-fg);
    background: var(--gh-bg-elev);
  }
  .filehdr {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .hunkhdr {
    padding-left: var(--gh-space-2);
  }
</style>
