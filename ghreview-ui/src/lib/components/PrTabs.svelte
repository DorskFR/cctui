<script lang="ts">
  import {
    PR_CONTENT_TAB_LABELS,
    PR_CONTENT_TABS,
    type PrContentTab,
  } from "../stores/pr-tabs-core";

  interface Props {
    active: PrContentTab;
    counts?: Partial<Record<PrContentTab, number>>;
    onselect: (tab: PrContentTab) => void;
  }
  let { active, counts = {}, onselect }: Props = $props();
</script>

<div class="prtabs" role="tablist" aria-label="Pull request content">
  {#each PR_CONTENT_TABS as tab (tab)}
    <button
      type="button"
      role="tab"
      aria-selected={active === tab}
      class:active={active === tab}
      onclick={() => onselect(tab)}
    >
      {PR_CONTENT_TAB_LABELS[tab]}
      {#if counts[tab] !== undefined}<span class="count">{counts[tab]}</span>{/if}
    </button>
  {/each}
</div>

<style>
  .prtabs {
    display: flex;
    gap: var(--gh-space-1);
    padding: 0 var(--gh-space-3);
    border-bottom: 1px solid var(--gh-border);
  }
  button {
    display: inline-flex;
    align-items: center;
    gap: var(--gh-space-1);
    background: transparent;
    border: none;
    border-bottom: 2px solid transparent;
    color: var(--gh-fg-muted);
    font-size: 13px;
    padding: var(--gh-space-2) var(--gh-space-2);
    cursor: pointer;
  }
  button:hover {
    color: var(--gh-fg);
  }
  button.active {
    color: var(--gh-fg);
    border-bottom-color: var(--gh-accent);
    font-weight: 600;
  }
  .count {
    font-size: 11px;
    font-family: var(--gh-mono);
    color: var(--gh-fg-muted);
    background: var(--gh-bg-inset);
    border-radius: 999px;
    padding: 0 6px;
  }
</style>
