<script lang="ts">
  import { IconButton, Tabs, type TabItem } from "@dorsk/tsumikit";
  import { pullPath } from "../router/route";
  import { router } from "../router/router.svelte";
  import { tabs } from "../stores/tabs.svelte";
  import StatusDot from "./StatusDot.svelte";

  const byId = $derived(new Map(tabs.tabs.map((t) => [t.id, t])));

  const items = $derived<TabItem[]>(
    tabs.tabs.map((tab) => ({
      id: tab.id,
      label: tab.title || `${tab.owner}/${tab.repo}`,
      title: `${tab.owner}/${tab.repo} #${tab.number} — ${tab.title}`,
    })),
  );

  const activeId = $derived(
    router.current.name === "pull"
      ? `pr-${router.current.owner}-${router.current.repo}-${router.current.number}`
      : (tabs.activeId ?? tabs.tabs[0]?.id),
  );

  function activate(id: string): void {
    const tab = byId.get(id);
    if (!tab) return;
    tabs.activate(id);
    router.navigate(pullPath(tab.owner, tab.repo, tab.number));
  }

  function close(id: string): void {
    tabs.close(id);
    if (tabs.tabs.length === 0) router.navigate("/");
  }

  function closeAll(): void {
    for (const tab of [...tabs.tabs]) tabs.close(tab.id);
    router.navigate("/");
  }
</script>

{#snippet leading(item: TabItem)}
  {@const tab = byId.get(item.id)}
  {#if tab}
    <StatusDot pr={tab.status.pr} ci={tab.status.ci} />
    <span class="tab-num">#{tab.number}</span>
  {/if}
{/snippet}

{#snippet closeAllAction()}
  <IconButton
    icon="trash"
    label="Close all tabs"
    variant="ghost"
    size={16}
    hoverDanger
    onclick={closeAll}
  />
{/snippet}

<nav class="tabbar">
  <Tabs
    tabs={items}
    bind:value={() => activeId, (id) => id && activate(id)}
    label="Open pull requests"
    closable
    onclose={close}
    {leading}
    actions={tabs.tabs.length > 1 ? closeAllAction : undefined}
  />
</nav>

<style>
  .tabbar {
    --tab-max-width: 220px;
    display: block;
    min-width: 0;
    background: var(--gh-bg-inset);
    padding: 0 var(--gh-space-2);
    z-index: var(--gh-z-header);
  }
  .tab-num {
    flex: none;
    font-family: var(--gh-mono);
    font-size: var(--fs-xs);
    color: var(--gh-fg-muted);
  }
</style>
