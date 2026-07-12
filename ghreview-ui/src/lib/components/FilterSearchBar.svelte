<script lang="ts">
  import type { PrFilterCriteria } from "../filter/prfilter";

  interface Props {
    criteria: PrFilterCriteria;
    repos: string[];
    authors: string[];
    labels: string[];
  }

  let { criteria = $bindable(), repos, authors, labels }: Props = $props();
</script>

<div class="bar">
  <input
    class="search"
    type="search"
    placeholder="Search title, author, repo, #number, label…"
    bind:value={criteria.text}
  />

  <div class="chips">
    <button class:on={criteria.relation === "all"} onclick={() => (criteria.relation = "all")}>
      All
    </button>
    <button class:on={criteria.relation === "review"} onclick={() => (criteria.relation = "review")}>
      Review
    </button>
    <button
      class:on={criteria.relation === "authored"}
      onclick={() => (criteria.relation = "authored")}
    >
      Authored
    </button>
  </div>

  <div class="selects">
    <select bind:value={criteria.state} aria-label="State">
      <option value="all">Any state</option>
      <option value="open">Open</option>
      <option value="draft">Draft</option>
      <option value="merged">Merged</option>
      <option value="closed">Closed</option>
    </select>

    <select bind:value={criteria.repo} aria-label="Repository">
      <option value="">All repos</option>
      {#each repos as r (r)}
        <option value={r}>{r}</option>
      {/each}
    </select>

    <select bind:value={criteria.author} aria-label="Author">
      <option value="">Any author</option>
      {#each authors as a (a)}
        <option value={a}>{a}</option>
      {/each}
    </select>

    <select bind:value={criteria.label} aria-label="Label" disabled={labels.length === 0}>
      <option value="">Any label</option>
      {#each labels as l (l)}
        <option value={l}>{l}</option>
      {/each}
    </select>
  </div>
</div>

<style>
  .bar {
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-2);
    margin-bottom: var(--gh-space-3);
  }
  .search {
    width: 100%;
    box-sizing: border-box;
    background: var(--gh-bg-elev);
    border: 1px solid var(--gh-border);
    color: var(--gh-fg);
    border-radius: var(--gh-radius);
    padding: var(--gh-space-2) var(--gh-space-3);
    font-size: 13px;
  }
  .chips {
    display: flex;
    gap: var(--gh-space-1);
  }
  .chips button {
    flex: 1;
    background: var(--gh-bg-elev);
    border: 1px solid var(--gh-border);
    color: var(--gh-fg-muted);
    border-radius: 999px;
    padding: 2px 12px;
    cursor: pointer;
    font-size: 12px;
  }
  .chips button.on {
    background: var(--gh-accent);
    color: white;
    border-color: var(--gh-accent);
  }
  .selects {
    display: flex;
    flex-wrap: wrap;
    gap: var(--gh-space-1);
  }
  select {
    flex: 1 1 45%;
    min-width: 0;
    background: var(--gh-bg-elev);
    border: 1px solid var(--gh-border);
    color: var(--gh-fg);
    border-radius: var(--gh-radius);
    padding: 2px 8px;
    font-size: 12px;
  }
  select:disabled {
    opacity: 0.5;
  }
</style>
