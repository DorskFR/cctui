<script lang="ts">
  import { ResizablePanel } from "@dorsk/tsumikit";
  import { router } from "../router/router.svelte";
  import PrList from "./PrList.svelte";
  import PrView from "./PrView.svelte";
  import TabBar from "./TabBar.svelte";

  const WIDTH_KEY = "ghreview:masterWidth";
  const route = $derived(router.current);
</script>

<div class="md">
  <ResizablePanel
    side="left"
    label="Pull request list"
    width={320}
    minWidth={220}
    maxWidth={720}
    resizeStep={24}
    widthKey={WIDTH_KEY}
  >
    {#snippet panel()}
      <PrList />
    {/snippet}

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
  </ResizablePanel>
</div>

<style>
  .md {
    position: relative;
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: grid;
  }
  .detail {
    position: relative;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    height: 100%;
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
