<script lang="ts">
  import { Tabs, type TabItem } from "@dorsk/tsumikit";
  import {
    isPrContentTab,
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

  const items = $derived<TabItem[]>(
    PR_CONTENT_TABS.map((tab) => ({
      id: tab,
      label: PR_CONTENT_TAB_LABELS[tab],
      count: counts[tab],
    })),
  );
</script>

<div class="prtabs">
  <Tabs
    tabs={items}
    bind:value={() => active, (id) => isPrContentTab(id) && onselect(id)}
    label="Pull request content"
  />
</div>

<style>
  .prtabs {
    padding: 0 var(--gh-space-3);
  }
</style>
