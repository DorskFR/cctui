<script lang="ts">
  import { QueryClientProvider } from "@tanstack/svelte-query";
  import { onMount } from "svelte";
  import { queryClient } from "../api/queries";
  import { subscribeSse } from "../api/sse";
  import { router } from "../router/router.svelte";
  import { layout } from "../stores/layout.svelte";
  import Bookmarklet from "./Bookmarklet.svelte";
  import Inbox from "./Inbox.svelte";
  import MasterDetail from "./MasterDetail.svelte";
  import PrList from "./PrList.svelte";
  import PrView from "./PrView.svelte";
  import TabBar from "./TabBar.svelte";
  import TopBar from "./TopBar.svelte";

  onMount(() => {
    const handle = subscribeSse(queryClient);
    return () => handle.close();
  });

  const route = $derived(router.current);
</script>

<QueryClientProvider client={queryClient}>
  <TopBar />
  <div class="modebar">
    <div class="spacer"></div>
    <span class="switch" role="group" aria-label="Layout mode">
      <button
        type="button"
        class:on={layout.mode === "panels"}
        onclick={() => layout.setMode("panels")}
      >Panels</button>
      <button
        type="button"
        class:on={layout.mode === "tabs"}
        onclick={() => layout.setMode("tabs")}
      >Tabs</button>
    </span>
  </div>

  {#if layout.mode === "panels"}
    <MasterDetail />
  {:else}
    <TabBar />
    <main class="content">
      {#if route.name === "home"}
        <PrList />
      {:else if route.name === "inbox"}
        <Inbox />
      {:else if route.name === "bookmarklet"}
        <Bookmarklet />
      {:else if route.name === "pull"}
        {#key `${route.owner}/${route.repo}/${route.number}`}
          <PrView owner={route.owner} repo={route.repo} number={route.number} />
        {/key}
      {:else}
        <div class="empty">Not found: {route.path}</div>
      {/if}
    </main>
  {/if}
</QueryClientProvider>

<style>
  .modebar {
    display: flex;
    align-items: center;
    gap: var(--gh-space-2);
    padding: var(--gh-space-1) var(--gh-space-3);
    background: var(--gh-bg-inset);
    border-bottom: 1px solid var(--gh-border);
  }
  .spacer {
    flex: 1;
  }
  .switch {
    display: inline-flex;
    gap: 2px;
  }
  .switch button {
    background: var(--gh-bg-elev);
    border: 1px solid var(--gh-border);
    color: var(--gh-fg-muted);
    cursor: pointer;
    font-size: 11px;
    padding: 2px 10px;
  }
  .switch button:first-child {
    border-radius: 999px 0 0 999px;
  }
  .switch button:last-child {
    border-radius: 0 999px 999px 0;
  }
  .switch button.on {
    background: var(--gh-accent);
    color: white;
    border-color: var(--gh-accent);
  }
  .content {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }
  .empty {
    padding: var(--gh-space-4);
    color: var(--gh-fg-muted);
  }
</style>
