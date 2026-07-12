<script lang="ts">
  import { QueryClientProvider } from "@tanstack/svelte-query";
  import { onMount } from "svelte";
  import { queryClient } from "../api/queries";
  import { subscribeSse } from "../api/sse";
  import { router } from "../router/router.svelte";
  import Bookmarklet from "./Bookmarklet.svelte";
  import Inbox from "./Inbox.svelte";
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
</QueryClientProvider>

<style>
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
