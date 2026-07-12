<script lang="ts">
  import { onDestroy } from "svelte";
  import type { DiffModel } from "../diff/parse";
  import type { NavIndex } from "../diff/navindex";
  import {
    anchorScreenY,
    clampScroll,
    hitTest,
    ROW_HEIGHT,
    scrollToRow,
  } from "../diff/canvas/layout";
  import { type Ctx2D, paint } from "../diff/canvas/paint";
  import {
    type LineSelection,
    normalizeSelection,
    rangeToClipboardText,
    selectionEvent,
    type SelectionEvent,
  } from "../diff/canvas/selection";
  import { themeTokens, type ThemeTokens } from "../theme/theme";

  interface Props {
    model: DiffModel;
    nav: NavIndex;
    focusRow: number;
    onFocusRow: (rowIndex: number) => void;
    onSelectRange?: (event: SelectionEvent) => void;
  }
  let { model, focusRow, onFocusRow, onSelectRange }: Props = $props();

  const FONT_SIZE = 12;
  const FONT_FAMILY =
    'ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace';

  let wrap = $state<HTMLDivElement | null>(null);
  let canvasEl = $state<HTMLCanvasElement | null>(null);
  let viewportW = $state(0);
  let viewportH = $state(600);
  let scrollTop = $state(0);
  let selection = $state<LineSelection | null>(null);
  let draft = $state<{ start: number; end: number } | null>(null);

  let tokens: ThemeTokens = themeTokens();
  let worker: Worker | null = null;
  let ctx: Ctx2D | null = null;
  let mainCanvasSized = false;
  let dragMode: "none" | "scroll" | "select" = "none";
  let dragLastY = 0;
  let dragLastT = 0;
  let velocity = 0;
  let momentumRaf = 0;
  let paintScheduled = false;

  function frameParams() {
    return {
      model,
      tokens,
      scrollTop,
      viewportWidth: viewportW,
      viewportHeight: viewportH,
      dpr: window.devicePixelRatio || 1,
      rowHeight: ROW_HEIGHT,
      focusRow,
      selection,
      fontFamily: FONT_FAMILY,
      fontSize: FONT_SIZE,
    };
  }

  function render(): void {
    if (viewportW === 0 || viewportH === 0) return;
    const p = frameParams();
    if (worker) {
      worker.postMessage({
        type: "frame",
        tokens: p.tokens,
        scrollTop: p.scrollTop,
        viewportWidth: p.viewportWidth,
        viewportHeight: p.viewportHeight,
        dpr: p.dpr,
        rowHeight: p.rowHeight,
        focusRow: p.focusRow,
        selection: p.selection,
        fontFamily: p.fontFamily,
        fontSize: p.fontSize,
      });
      return;
    }
    const el = canvasEl;
    if (!ctx || !el) return;
    const w = Math.round(p.viewportWidth * p.dpr);
    const h = Math.round(p.viewportHeight * p.dpr);
    if (!mainCanvasSized || el.width !== w) el.width = w;
    if (!mainCanvasSized || el.height !== h) el.height = h;
    mainCanvasSized = true;
    paint(ctx, p);
  }

  function schedulePaint(): void {
    if (paintScheduled) return;
    paintScheduled = true;
    requestAnimationFrame(() => {
      paintScheduled = false;
      render();
    });
  }

  function setup(el: HTMLCanvasElement): void {
    const supportsOffscreen =
      typeof el.transferControlToOffscreen === "function" && typeof Worker !== "undefined";
    if (supportsOffscreen) {
      try {
        worker = new Worker(new URL("../diff/canvas/paint.worker.ts", import.meta.url), {
          type: "module",
        });
        const offscreen = el.transferControlToOffscreen();
        worker.postMessage({ type: "init", canvas: offscreen }, [offscreen]);
        worker.postMessage({ type: "model", model: $state.snapshot(model) });
        return;
      } catch {
        worker = null;
      }
    }
    ctx = el.getContext("2d") as unknown as Ctx2D | null;
  }

  $effect(() => {
    const el = canvasEl;
    if (!el || ctx || worker) return;
    setup(el);
  });

  $effect(() => {
    if (worker) worker.postMessage({ type: "model", model: $state.snapshot(model) });
    mainCanvasSized = false;
    scrollTop = clampScroll(scrollTop, model, viewportH);
    schedulePaint();
  });

  $effect(() => {
    void scrollTop;
    void focusRow;
    void selection;
    void viewportW;
    void viewportH;
    schedulePaint();
  });

  $effect(() => {
    scrollTop = scrollToRow(focusRow, scrollTop, viewportH, model);
  });

  $effect(() => {
    const target = document.documentElement;
    const obs = new MutationObserver(() => {
      tokens = themeTokens();
      schedulePaint();
    });
    obs.observe(target, { attributes: true, attributeFilter: ["data-theme"] });
    return () => obs.disconnect();
  });

  function stopMomentum(): void {
    if (momentumRaf) cancelAnimationFrame(momentumRaf);
    momentumRaf = 0;
    velocity = 0;
  }

  function momentum(): void {
    if (Math.abs(velocity) < 0.08) {
      stopMomentum();
      return;
    }
    scrollTop = clampScroll(scrollTop + velocity, model, viewportH);
    velocity *= 0.94;
    momentumRaf = requestAnimationFrame(momentum);
  }

  function onWheel(e: WheelEvent): void {
    e.preventDefault();
    stopMomentum();
    scrollTop = clampScroll(scrollTop + e.deltaY, model, viewportH);
  }

  function localPoint(e: PointerEvent): { x: number; y: number } {
    const rect = canvasEl?.getBoundingClientRect();
    return { x: e.clientX - (rect?.left ?? 0), y: e.clientY - (rect?.top ?? 0) };
  }

  function onPointerDown(e: PointerEvent): void {
    if (!canvasEl) return;
    stopMomentum();
    const { x, y } = localPoint(e);
    const hit = hitTest(model, x, y, scrollTop);
    canvasEl.setPointerCapture(e.pointerId);
    if (hit && (hit.region === "oldGutter" || hit.region === "newGutter")) {
      dragMode = "select";
      selection = { anchor: hit.rowIndex, head: hit.rowIndex };
      draft = null;
    } else {
      dragMode = "scroll";
      dragLastY = e.clientY;
      dragLastT = e.timeStamp;
      velocity = 0;
    }
  }

  function onPointerMove(e: PointerEvent): void {
    if (dragMode === "none") return;
    if (dragMode === "scroll") {
      const dy = e.clientY - dragLastY;
      scrollTop = clampScroll(scrollTop - dy, model, viewportH);
      const dt = e.timeStamp - dragLastT;
      if (dt > 0) velocity = (-dy / dt) * 16;
      dragLastY = e.clientY;
      dragLastT = e.timeStamp;
      return;
    }
    const { x, y } = localPoint(e);
    const hit = hitTest(model, x, y, scrollTop);
    if (hit && selection) selection = { anchor: selection.anchor, head: hit.rowIndex };
  }

  function onPointerUp(e: PointerEvent): void {
    canvasEl?.releasePointerCapture(e.pointerId);
    if (dragMode === "scroll") {
      if (Math.abs(velocity) > 0.5) momentumRaf = requestAnimationFrame(momentum);
    } else if (dragMode === "select" && selection) {
      const norm = normalizeSelection(selection);
      if (norm.start === norm.end) {
        onFocusRow(norm.start);
        selection = null;
      } else {
        draft = norm;
        onSelectRange?.(selectionEvent(model, selection));
      }
    }
    dragMode = "none";
  }

  async function copySelection(): Promise<void> {
    if (!selection) return;
    try {
      await navigator.clipboard.writeText(rangeToClipboardText(model, selection));
    } catch {
      // clipboard unavailable (insecure context / denied); range-copy is best-effort
    }
  }

  function onKeydown(e: KeyboardEvent): void {
    if ((e.metaKey || e.ctrlKey) && e.key === "c" && selection) {
      const norm = normalizeSelection(selection);
      if (norm.start !== norm.end) {
        e.preventDefault();
        void copySelection();
      }
    }
  }

  function dismissDraft(): void {
    draft = null;
    selection = null;
  }

  onDestroy(() => {
    stopMomentum();
    worker?.terminate();
  });
</script>

<div
  class="canvas-wrap"
  bind:this={wrap}
  bind:clientWidth={viewportW}
  bind:clientHeight={viewportH}
>
  <canvas
    bind:this={canvasEl}
    style:width="{viewportW}px"
    style:height="{viewportH}px"
    onwheel={onWheel}
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    onkeydown={onKeydown}
    tabindex="0"
    role="grid"
    aria-label="Diff"
  ></canvas>

  <div class="overlay" aria-hidden={draft === null}>
    {#if draft}
      <div class="draft" style:top="{anchorScreenY(draft.end + 1, scrollTop) + 2}px">
        <div class="draft-head">
          Comment on lines {model.rows[draft.start]?.newLine ??
            model.rows[draft.start]?.oldLine ??
            draft.start + 1}–{model.rows[draft.end]?.newLine ??
            model.rows[draft.end]?.oldLine ??
            draft.end + 1}
          <button type="button" class="x" onclick={dismissDraft} aria-label="Cancel">×</button>
        </div>
        <textarea placeholder="Leave a comment…" rows="3"></textarea>
        <div class="draft-actions">
          <button type="button" onclick={() => void copySelection()}>Copy lines</button>
        </div>
      </div>
    {/if}
  </div>
</div>

<style>
  .canvas-wrap {
    position: relative;
    height: 100%;
    width: 100%;
    overflow: hidden;
    background: var(--gh-bg);
  }
  canvas {
    display: block;
    touch-action: none;
    outline: none;
    cursor: default;
  }
  .overlay {
    position: absolute;
    inset: 0;
    pointer-events: none;
  }
  .draft {
    position: absolute;
    left: 110px;
    right: 12px;
    pointer-events: auto;
    background: var(--gh-bg-elev);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
    padding: var(--gh-space-2);
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.4);
  }
  .draft-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    font-size: 12px;
    color: var(--gh-fg-muted);
    margin-bottom: var(--gh-space-1);
  }
  .draft textarea {
    width: 100%;
    box-sizing: border-box;
    font-family: var(--gh-mono);
    font-size: 12px;
    background: var(--gh-bg);
    color: var(--gh-fg);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius-sm);
    padding: var(--gh-space-1);
    resize: vertical;
  }
  .draft-actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--gh-space-2);
    margin-top: var(--gh-space-1);
  }
  .draft-actions button,
  .draft .x {
    font-size: 12px;
    color: var(--gh-fg);
    background: var(--gh-bg-inset);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius-sm);
    padding: 2px 8px;
    cursor: pointer;
  }
  .draft .x {
    border: none;
    background: none;
    padding: 0 4px;
    color: var(--gh-fg-muted);
  }
</style>
