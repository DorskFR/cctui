<script lang="ts">
  import { QueryClientProvider } from "@tanstack/svelte-query";
  import { getContext, onMount } from "svelte";
  import { Button, SegmentedControl, Select } from "@dorsk/tsumikit";
  import { queryClient } from "../api/queries";
  import { subscribeSse } from "../api/sse";
  import { EMBED_KEY, type EmbedContext } from "../embed/context";
  import { router } from "../router/router.svelte";
  import { layout } from "../stores/layout.svelte";
  import { currentTheme, setTheme, type Theme, THEME_LABELS, THEMES } from "../theme/theme";
  import Bookmarklet from "./Bookmarklet.svelte";
  import Inbox from "./Inbox.svelte";
  import MasterDetail from "./MasterDetail.svelte";
  import PrList from "./PrList.svelte";
  import PrView from "./PrView.svelte";
  import SubscribeMenu from "./SubscribeMenu.svelte";
  import TabBar from "./TabBar.svelte";

  const embedded = getContext<EmbedContext | undefined>(EMBED_KEY)?.embedded ?? false;

  onMount(() => {
    const handle = subscribeSse(queryClient);
    return () => handle.close();
  });

  const route = $derived(router.current);

  const viewOptions = [
    { value: "prs", label: "Pull requests" },
    { value: "inbox", label: "Inbox" },
  ];
  let view = $state(router.current.name === "inbox" ? "inbox" : "prs");
  $effect(() => {
    view = route.name === "inbox" ? "inbox" : "prs";
  });
  $effect(() => {
    const cur = route.name === "inbox" ? "inbox" : "prs";
    if (view !== cur) router.navigate(view === "inbox" ? "/inbox" : "/");
  });

  const modeOptions = [
    { value: "panels", label: "Panels" },
    { value: "tabs", label: "Tabs" },
  ];
  let mode = $state(layout.mode);
  $effect(() => {
    if (mode !== layout.mode) layout.setMode(mode as typeof layout.mode);
  });

  let theme = $state<Theme>(currentTheme());
  function onThemeChange(e: Event): void {
    theme = (e.currentTarget as HTMLSelectElement).value as Theme;
    setTheme(theme);
  }
</script>

<QueryClientProvider client={queryClient}>
  <header class="toolbar">
    <SegmentedControl options={viewOptions} bind:value={view} size="sm" label="View" />
    <SubscribeMenu />
    <Button variant="ghost" size="sm" onclick={() => router.navigate("/bookmarklet")}>
      Bookmarklet
    </Button>
    <div class="spacer"></div>
    <SegmentedControl options={modeOptions} bind:value={mode} size="sm" label="Layout mode" />
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
  {:else if route.name === "bookmarklet"}
    <main class="content"><Bookmarklet /></main>
  {:else if layout.mode === "panels"}
    <MasterDetail />
  {:else}
    <TabBar />
    <main class="content">
      {#if route.name === "pull"}
        {#key `${route.owner}/${route.repo}/${route.number}`}
          <PrView owner={route.owner} repo={route.repo} number={route.number} />
        {/key}
      {:else if route.name === "notfound"}
        <div class="empty">Not found: {route.path}</div>
      {:else}
        <PrList />
      {/if}
    </main>
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
  .empty {
    padding: var(--gh-space-4);
    color: var(--gh-fg-muted);
  }
</style>
