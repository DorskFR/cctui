<script lang="ts">
  import { IconButton } from "@dorsk/tsumikit";
  import { pullPath } from "../router/route";
  import { router } from "../router/router.svelte";
  import { tabs } from "../stores/tabs.svelte";
  import StatusDot from "./StatusDot.svelte";

  function activate(owner: string, repo: string, number: number, id: string): void {
    tabs.activate(id);
    router.navigate(pullPath(owner, repo, number));
  }

  function close(id: string): void {
    const wasActive = tabs.activeId === id;
    tabs.close(id);
    if (wasActive) {
      const next = tabs.tabs.find((t) => t.id === tabs.activeId);
      if (next) router.navigate(pullPath(next.owner, next.repo, next.number));
      else router.navigate("/");
    }
  }

  function closeAll(): void {
    for (const tab of [...tabs.tabs]) tabs.close(tab.id);
    router.navigate("/");
  }

  function onAux(e: MouseEvent, id: string): void {
    if (e.button === 1) {
      e.preventDefault();
      close(id);
    }
  }

  const activeId = $derived(
    router.current.name === "pull"
      ? `pr-${router.current.owner}-${router.current.repo}-${router.current.number}`
      : tabs.activeId,
  );
</script>

<nav class="tabbar">
  <div class="tabs">
    {#each tabs.tabs as tab (tab.id)}
      <div
        class="tab"
        class:active={tab.id === activeId}
        role="tab"
        tabindex="0"
        aria-selected={tab.id === activeId}
        onclick={() => activate(tab.owner, tab.repo, tab.number, tab.id)}
        onauxclick={(e) => onAux(e, tab.id)}
        onkeydown={(e) => e.key === "Enter" && activate(tab.owner, tab.repo, tab.number, tab.id)}
      >
        <StatusDot pr={tab.status.pr} ci={tab.status.ci} />
        <span class="label" title="{tab.owner}/{tab.repo} #{tab.number} — {tab.title}">
          <span class="tab-num">#{tab.number}</span>
          <span class="tab-title">{tab.title || `${tab.owner}/${tab.repo}`}</span>
        </span>
        <IconButton
          icon="x"
          label="Close tab"
          variant="ghost"
          size={14}
          inline
          hoverDanger
          onclick={(e) => {
            e.stopPropagation();
            close(tab.id);
          }}
        />
      </div>
    {/each}
  </div>
  {#if tabs.tabs.length > 1}
    <div class="actions">
      <IconButton
        icon="trash"
        label="Close all tabs"
        variant="ghost"
        size={16}
        hoverDanger
        onclick={closeAll}
      />
    </div>
  {/if}
</nav>

<style>
  .tabbar {
    display: flex;
    align-items: stretch;
    gap: 2px;
    background: var(--gh-bg-inset);
    border-bottom: 1px solid var(--gh-border);
    padding: 0 var(--gh-space-2);
    overflow-x: auto;
    z-index: var(--gh-z-header);
  }
  .tabs {
    display: flex;
    align-items: stretch;
  }
  .tab {
    display: flex;
    align-items: center;
    gap: var(--gh-space-2);
    padding: var(--gh-space-2) var(--gh-space-2) var(--gh-space-2) var(--gh-space-3);
    border-right: 1px solid var(--gh-border-muted);
    color: var(--gh-fg-muted);
    cursor: pointer;
    max-width: 220px;
    white-space: nowrap;
  }
  .tab.active {
    background: var(--gh-bg);
    color: var(--gh-fg);
    box-shadow: inset 0 -2px 0 var(--gh-accent);
  }
  .label {
    display: inline-flex;
    align-items: baseline;
    gap: 5px;
    overflow: hidden;
    font-size: var(--fs-xs);
  }
  .tab-num {
    flex: none;
    font-family: var(--gh-mono);
    color: var(--gh-fg-muted);
  }
  .tab-title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .actions {
    display: flex;
    align-items: center;
    margin-left: auto;
    padding-left: var(--gh-space-2);
  }
</style>
