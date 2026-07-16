<script lang="ts">
  import { SegmentedControl } from "@dorsk/tsumikit";
  import {
    PR_CONTENT_TAB_LABELS,
    PR_CONTENT_TABS,
    isPrContentTab,
    type PrContentTab,
  } from "../stores/pr-tabs-core";

  interface Props {
    active: PrContentTab;
    counts?: Partial<Record<PrContentTab, number>>;
    onselect: (tab: PrContentTab) => void;
  }
  let { active, counts = {}, onselect }: Props = $props();

  const options = $derived(
    PR_CONTENT_TABS.map((tab) => ({
      value: tab,
      label: PR_CONTENT_TAB_LABELS[tab],
      count: counts[tab],
    })),
  );

  let selected = $state<string>("");
  $effect(() => {
    selected = active;
  });
  $effect(() => {
    if (isPrContentTab(selected) && selected !== active) onselect(selected);
  });
</script>

<div class="prtabs">
  <SegmentedControl
    {options}
    bind:value={selected}
    variant="pill"
    size="sm"
    label="Pull request content"
  />
</div>

<style>
  .prtabs {
    padding: 0 var(--gh-space-3);
    border-bottom: 1px solid var(--gh-border);
  }
</style>
