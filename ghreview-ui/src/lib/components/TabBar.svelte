<script lang="ts">
  import { pullPath } from "../router/route";
  import { router } from "../router/router.svelte";
  import { tabs } from "../stores/tabs.svelte";
  import StatusDot from "./StatusDot.svelte";

  function activate(owner: string, repo: string, number: number, id: string): void {
    tabs.activate(id);
    router.navigate(pullPath(owner, repo, number));
  }

  function close(e: MouseEvent, id: string): void {
    e.stopPropagation();
    e.preventDefault();
    const wasActive = tabs.activeId === id;
    tabs.close(id);
    if (wasActive) {
      const next = tabs.tabs.find((t) => t.id === tabs.activeId);
      if (next) router.navigate(pullPath(next.owner, next.repo, next.number));
      else router.navigate("/");
    }
  }

  function onAux(e: MouseEvent, id: string): void {
    if (e.button === 1) close(e, id);
  }

  const activeId = $derived(
    router.current.name === "pull"
      ? `pr-${router.current.owner}-${router.current.repo}-${router.current.number}`
      : tabs.activeId,
  );
</script>

<nav class="tabbar">
  <button class="home" class:active={router.current.name === "home"} onclick={() => router.navigate("/")}>
    Home
  </button>
  <button class="home" class:active={router.current.name === "inbox"} onclick={() => router.navigate("/inbox")}>
    Inbox
  </button>
  <div class="tabs">
    {#each tabs.tabs as tab (tab.id)}
      <div
        class="tab"
        class:active={tab.id === activeId}
        role="tab"
        tabindex="0"
        onclick={() => activate(tab.owner, tab.repo, tab.number, tab.id)}
        onauxclick={(e) => onAux(e, tab.id)}
        onkeydown={(e) => e.key === "Enter" && activate(tab.owner, tab.repo, tab.number, tab.id)}
      >
        <StatusDot pr={tab.status.pr} ci={tab.status.ci} />
        <span class="label" title={tab.title}>{tab.owner}/{tab.repo} #{tab.number}</span>
        <button class="x" title="Close tab" onclick={(e) => close(e, tab.id)}>×</button>
      </div>
    {/each}
  </div>
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
  .home {
    background: transparent;
    border: none;
    color: var(--gh-fg-muted);
    padding: var(--gh-space-2) var(--gh-space-3);
    cursor: pointer;
    font-size: 12px;
  }
  .home.active {
    color: var(--gh-fg);
    box-shadow: inset 0 -2px 0 var(--gh-accent);
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
    overflow: hidden;
    text-overflow: ellipsis;
    font-size: 12px;
  }
  .x {
    background: transparent;
    border: none;
    color: var(--gh-fg-subtle);
    cursor: pointer;
    font-size: 14px;
    line-height: 1;
    padding: 0 2px;
    border-radius: var(--gh-radius-sm);
  }
  .x:hover {
    color: var(--gh-fg);
    background: var(--gh-border-muted);
  }
</style>
