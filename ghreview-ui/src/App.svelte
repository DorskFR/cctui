<script lang="ts">
  import { QueryClientProvider } from "@tanstack/svelte-query";
  import { onMount } from "svelte";
  import { getToken } from "./lib/api/config";
  import { queryClient } from "./lib/api/queries";
  import { subscribeSse } from "./lib/api/sse";
  import AuthGate from "./lib/components/AuthGate.svelte";
  import Bookmarklet from "./lib/components/Bookmarklet.svelte";
  import Inbox from "./lib/components/Inbox.svelte";
  import PrList from "./lib/components/PrList.svelte";
  import PrView from "./lib/components/PrView.svelte";
  import TabBar from "./lib/components/TabBar.svelte";
  import TopBar from "./lib/components/TopBar.svelte";
  import { router } from "./lib/router/router.svelte";

  let hasToken = $state(!!getToken());

  onMount(() => {
    if (!hasToken) return;
    const handle = subscribeSse(queryClient);
    return () => handle.close();
  });

  const route = $derived(router.current);
</script>

<QueryClientProvider client={queryClient}>
  {#if !hasToken}
    <AuthGate onauthed={() => (hasToken = true)} />
  {:else}
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
  {/if}
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
