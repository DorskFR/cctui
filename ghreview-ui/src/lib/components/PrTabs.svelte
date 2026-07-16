<script lang="ts">
  import { Badge, Button } from "@dorsk/tsumikit";
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
    {@const count = counts[tab]}
    <Button
      variant="ghost"
      size="sm"
      class={tab === active ? "pr-tab pr-tab-active" : "pr-tab"}
      role="tab"
      aria-selected={tab === active}
      onclick={() => onselect(tab)}
    >
      {PR_CONTENT_TAB_LABELS[tab]}
      {#if count != null}<Badge tone="neutral" size="sm" mono>{count}</Badge>{/if}
    </Button>
  {/each}
</div>

<style>
  .prtabs {
    display: flex;
    align-items: stretch;
    gap: 2px;
    padding: 0 var(--gh-space-3);
    border-bottom: 1px solid var(--gh-border);
  }
  .prtabs :global(.pr-tab) {
    border-radius: 0;
    color: var(--gh-fg-muted);
    box-shadow: inset 0 -2px 0 transparent;
  }
  .prtabs :global(.pr-tab-active) {
    color: var(--gh-fg);
    box-shadow: inset 0 -2px 0 var(--gh-accent);
  }
</style>
