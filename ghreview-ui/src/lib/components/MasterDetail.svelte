<script lang="ts">
  import { Button } from "@dorsk/tsumikit";
  import { router } from "../router/router.svelte";
  import { layout } from "../stores/layout.svelte";
  import PrList from "./PrList.svelte";
  import PrView from "./PrView.svelte";
  import TabBar from "./TabBar.svelte";

  const route = $derived(router.current);

  const WIDTH_KEY = "ghreview:masterWidth";
  const MIN = 220;

  function loadWidth(): number {
    const n = Number(localStorage.getItem(WIDTH_KEY));
    return Number.isFinite(n) && n >= MIN ? n : 320;
  }

  let masterW = $state(loadWidth());
  let container = $state<HTMLElement | null>(null);
  let dragging = false;
  let rafId = 0;
  let lastX = 0;

  function maxWidth(): number {
    const w = container?.clientWidth ?? 900;
    return Math.max(MIN, Math.round(w * 0.6));
  }

  function startDrag(e: PointerEvent): void {
    dragging = true;
    lastX = e.clientX;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    e.preventDefault();
  }

  function onDrag(e: PointerEvent): void {
    if (!dragging) return;
    lastX = e.clientX;
    if (rafId) return;
    rafId = requestAnimationFrame(() => {
      rafId = 0;
      const left = container?.getBoundingClientRect().left ?? 0;
      masterW = Math.round(Math.max(MIN, Math.min(lastX - left, maxWidth())));
    });
  }

  function endDrag(e: PointerEvent): void {
    if (!dragging) return;
    dragging = false;
    if (rafId) {
      cancelAnimationFrame(rafId);
      rafId = 0;
    }
    try {
      (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
    } catch {
      /* already released */
    }
    localStorage.setItem(WIDTH_KEY, String(masterW));
  }

  const cols = $derived(layout.fullWidth ? "1fr" : `${masterW}px 6px 1fr`);
</script>

<div class="md" bind:this={container} style="grid-template-columns: {cols}">
  <aside class="master" aria-hidden={layout.fullWidth}>
    <PrList />
  </aside>
  {#if !layout.fullWidth}
    <div
      class="handle"
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize pull request list"
      onpointerdown={startDrag}
      onpointermove={onDrag}
      onpointerup={endDrag}
      onpointercancel={endDrag}
    ></div>
  {/if}
  <section class="detail">
    <div class="detail-bar">
      <TabBar />
      <Button
        variant="ghost"
        size="sm"
        aria-pressed={layout.fullWidth}
        title={layout.fullWidth ? "Show PR list" : "Full width"}
        onclick={() => layout.toggleFullWidth()}
      >
        {layout.fullWidth ? "⇥ List" : "⤢ Full width"}
      </Button>
    </div>
    <div class="detail-body">
      {#if route.name === "pull"}
        {#key `${route.owner}/${route.repo}/${route.number}`}
          <PrView owner={route.owner} repo={route.repo} number={route.number} />
        {/key}
      {:else if route.name === "notfound"}
        <div class="empty">Not found: {route.path}</div>
      {:else}
        <div class="empty">Select a pull request from the list.</div>
      {/if}
    </div>
  </section>
</div>

<style>
  .md {
    flex: 1;
    min-height: 0;
    display: grid;
  }
  .master {
    min-height: 0;
    overflow: auto;
    background: var(--gh-bg);
  }
  .md aside[aria-hidden="true"] {
    display: none;
  }
  .handle {
    width: 6px;
    cursor: col-resize;
    background: var(--gh-border);
    touch-action: none;
  }
  .handle:hover {
    background: var(--gh-accent);
  }
  .detail {
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .detail-bar {
    display: flex;
    align-items: stretch;
    border-bottom: 1px solid var(--gh-border);
  }
  .detail-bar :global(.tabbar) {
    flex: 1;
    border-bottom: none;
  }
  .detail-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }
  .empty {
    padding: var(--gh-space-4);
    color: var(--gh-fg-muted);
  }
</style>
