<script lang="ts">
  import { router } from "../router/router.svelte";
  import { layout } from "../stores/layout.svelte";
  import Bookmarklet from "./Bookmarklet.svelte";
  import Inbox from "./Inbox.svelte";
  import PrList from "./PrList.svelte";
  import PrView from "./PrView.svelte";
  import TabBar from "./TabBar.svelte";

  const route = $derived(router.current);
</script>

<div class="md" class:full={layout.fullWidth}>
  <aside class="master" aria-hidden={layout.fullWidth}>
    <PrList />
  </aside>
  <section class="detail">
    <div class="detail-bar">
      <TabBar />
      <button
        type="button"
        class="fullwidth"
        aria-pressed={layout.fullWidth}
        title={layout.fullWidth ? "Show PR list" : "Full width"}
        onclick={() => layout.toggleFullWidth()}
      >
        {layout.fullWidth ? "⇥ List" : "⤢ Full width"}
      </button>
    </div>
    <div class="detail-body">
      {#if route.name === "pull"}
        {#key `${route.owner}/${route.repo}/${route.number}`}
          <PrView owner={route.owner} repo={route.repo} number={route.number} />
        {/key}
      {:else if route.name === "inbox"}
        <Inbox />
      {:else if route.name === "bookmarklet"}
        <Bookmarklet />
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
    grid-template-columns: minmax(280px, 30%) 1fr;
  }
  .md.full {
    grid-template-columns: 0 1fr;
  }
  .master {
    min-height: 0;
    overflow: auto;
    border-right: 1px solid var(--gh-border);
    background: var(--gh-bg);
  }
  .md.full .master {
    display: none;
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
  .fullwidth {
    background: var(--gh-bg-inset);
    border: none;
    border-left: 1px solid var(--gh-border);
    color: var(--gh-fg-muted);
    cursor: pointer;
    font-size: 12px;
    padding: 0 var(--gh-space-3);
    white-space: nowrap;
  }
  .fullwidth:hover {
    color: var(--gh-fg);
  }
  .fullwidth[aria-pressed="true"] {
    color: var(--gh-fg);
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
