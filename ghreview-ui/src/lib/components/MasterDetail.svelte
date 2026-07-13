<script lang="ts">
  import { router } from "../router/router.svelte";
  import { layout } from "../stores/layout.svelte";
  import PrList from "./PrList.svelte";
  import PrView from "./PrView.svelte";
  import TabBar from "./TabBar.svelte";

  const route = $derived(router.current);
  const collapsed = $derived(layout.sidebarCollapsed);

  const WIDTH_KEY = "ghreview:masterWidth";
  const MIN = 220;
  const STEP = 24;

  function loadWidth(): number {
    const n = Number(localStorage.getItem(WIDTH_KEY));
    return Number.isFinite(n) && n >= MIN ? n : 320;
  }

  let masterW = $state(loadWidth());
  let container = $state<HTMLElement | null>(null);
  let dragging = $state(false);
  let rafId = 0;
  let lastX = 0;

  function maxWidth(): number {
    const w = container?.clientWidth ?? 900;
    return Math.max(MIN, Math.round(w * 0.6));
  }

  function setWidth(px: number): void {
    masterW = Math.round(Math.max(MIN, Math.min(px, maxWidth())));
  }

  function persistWidth(): void {
    localStorage.setItem(WIDTH_KEY, String(masterW));
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
      setWidth(lastX - left);
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
    persistWidth();
  }

  function onHandleKey(e: KeyboardEvent): void {
    if (e.key === "ArrowLeft") {
      setWidth(masterW - STEP);
      persistWidth();
      e.preventDefault();
    } else if (e.key === "ArrowRight") {
      setWidth(masterW + STEP);
      persistWidth();
      e.preventDefault();
    }
  }

  const cols = $derived(collapsed ? "1fr" : `${masterW}px 1fr`);
</script>

<div class="md" class:collapsed class:dragging bind:this={container} style="grid-template-columns: {cols}">
  {#if !collapsed}
    <aside class="master">
      <PrList />
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
      <div
        class="resize"
        role="separator"
        tabindex="0"
        aria-orientation="vertical"
        aria-label="Resize pull request list"
        aria-valuenow={masterW}
        aria-valuemin={MIN}
        aria-valuemax={maxWidth()}
        onpointerdown={startDrag}
        onpointermove={onDrag}
        onpointerup={endDrag}
        onpointercancel={endDrag}
        onkeydown={onHandleKey}
      >
        <button
          type="button"
          class="collapse-btn"
          title="Collapse list"
          aria-label="Collapse list"
          onpointerdown={(e) => e.stopPropagation()}
          onclick={() => layout.toggleSidebar()}
        >‹</button>
      </div>
    </aside>
  {:else}
    <button
      type="button"
      class="expand-btn"
      title="Show list"
      aria-label="Show list"
      onclick={() => layout.toggleSidebar()}
    >›</button>
  {/if}
  <section class="detail">
    <div class="detail-bar">
      <TabBar />
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
    position: relative;
    flex: 1;
    min-height: 0;
    display: grid;
  }
  .md.dragging {
    user-select: none;
    cursor: ew-resize;
  }
  .master {
    position: relative;
    min-height: 0;
    overflow: auto;
    background: var(--gh-bg);
    border-right: 1px solid var(--gh-border);
  }
  /* Splitter: matches Tsumikit AppShell — a thin invisible hit area with a
     centered grip pill that brightens to the accent on hover / drag. */
  .resize {
    position: absolute;
    top: 0;
    bottom: 0;
    right: 0;
    width: 10px;
    cursor: ew-resize;
    touch-action: none;
    z-index: 2;
  }
  .resize::after {
    content: "";
    position: absolute;
    top: 50%;
    right: 1px;
    transform: translateY(-50%);
    width: 3px;
    height: 28px;
    border-radius: 999px;
    background: var(--gh-border);
    transition: background 0.12s ease;
  }
  .resize:hover::after,
  .resize:focus-visible::after,
  .md.dragging .resize::after {
    background: var(--gh-accent);
  }
  .resize:focus-visible {
    outline: none;
  }
  .collapse-btn {
    position: absolute;
    top: var(--gh-space-2);
    right: -1px;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 22px;
    padding: 0;
    font-size: var(--fs-sm);
    line-height: 1;
    color: var(--gh-fg-muted);
    background: var(--gh-bg-elev);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius-sm);
    cursor: pointer;
  }
  .collapse-btn:hover {
    color: var(--gh-accent);
    border-color: var(--gh-accent);
  }
  .expand-btn {
    position: absolute;
    top: var(--gh-space-2);
    left: 0;
    z-index: 3;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 24px;
    padding: 0;
    font-size: var(--fs-sm);
    line-height: 1;
    color: var(--gh-fg-muted);
    background: var(--gh-bg-elev);
    border: 1px solid var(--gh-border);
    border-left: none;
    border-radius: 0 var(--gh-radius-sm) var(--gh-radius-sm) 0;
    cursor: pointer;
  }
  .expand-btn:hover {
    color: var(--gh-accent);
    border-color: var(--gh-accent);
  }
  .detail {
    position: relative;
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
