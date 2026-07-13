<script lang="ts">
  import { QueryClientProvider } from "@tanstack/svelte-query";
  import { getContext, onMount } from "svelte";
  import { Button, SegmentedControl, Select } from "@dorsk/tsumikit";
  import { api } from "../api/client";
  import { getAccount } from "../api/config";
  import { queryClient } from "../api/queries";
  import { subscribeSse } from "../api/sse";
  import { EMBED_KEY, type EmbedContext } from "../embed/context";
  import { router } from "../router/router.svelte";
  import { currentTheme, setTheme, type Theme, THEME_LABELS, THEMES } from "../theme/theme";
  import Inbox from "./Inbox.svelte";
  import MasterDetail from "./MasterDetail.svelte";
  import Subscriptions from "./Subscriptions.svelte";

  const embedded = getContext<EmbedContext | undefined>(EMBED_KEY)?.embedded ?? false;

  onMount(() => {
    const handle = subscribeSse(queryClient);
    return () => handle.close();
  });

  const route = $derived(router.current);

  const viewOptions = [
    { value: "prs", label: "Pull requests" },
    { value: "inbox", label: "Inbox" },
    { value: "subscriptions", label: "Subscriptions" },
  ];
  function viewOf(name: string): string {
    if (name === "inbox") return "inbox";
    if (name === "subscriptions") return "subscriptions";
    return "prs";
  }
  function pathOf(view: string): string {
    if (view === "inbox") return "/inbox";
    if (view === "subscriptions") return "/subscriptions";
    return "/";
  }
  let view = $state(viewOf(router.current.name));
  $effect(() => {
    view = viewOf(route.name);
  });
  $effect(() => {
    if (view !== viewOf(route.name)) router.navigate(pathOf(view));
  });

  let theme = $state<Theme>(currentTheme());
  function onThemeChange(e: Event): void {
    theme = (e.currentTarget as HTMLSelectElement).value as Theme;
    setTheme(theme);
  }

  let syncing = $state(false);
  async function runSync(): Promise<void> {
    if (syncing) return;
    syncing = true;
    try {
      await api.forceSync(getAccount() ?? undefined);
      await queryClient.invalidateQueries();
    } catch {
      /* surfaced via SSE / next poll */
    } finally {
      syncing = false;
    }
  }
</script>

<QueryClientProvider client={queryClient}>
  <header class="toolbar">
    <SegmentedControl options={viewOptions} bind:value={view} size="sm" label="View" />
    <Button size="sm" variant="default" disabled={syncing} onclick={runSync}>
      {syncing ? "Syncing…" : "Sync"}
    </Button>
    <div class="spacer"></div>
    {#if !embedded}
      <Select compact value={theme} onchange={onThemeChange} aria-label="Theme">
        {#each THEMES as t (t)}
          <option value={t}>{THEME_LABELS[t]}</option>
        {/each}
      </Select>
    {/if}
  </header>

  {#if route.name === "inbox"}
    <main class="content"><Inbox /></main>
  {:else if route.name === "subscriptions"}
    <main class="content"><Subscriptions /></main>
  {:else}
    <MasterDetail />
  {/if}
</QueryClientProvider>

<style>
  .toolbar {
    display: flex;
    align-items: center;
    gap: var(--gh-space-2);
    padding: var(--gh-space-1) var(--gh-space-3);
    background: var(--gh-bg-elev);
    border-bottom: 1px solid var(--gh-border);
    z-index: var(--gh-z-header);
  }
  .spacer {
    flex: 1;
  }
  .content {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }
</style>
