<script lang="ts">
  import { ciStateOf, type GithubPull } from "../api/types";
  import PrEmptyTab from "./PrEmptyTab.svelte";

  interface CheckRun {
    name?: string;
    status?: string;
    conclusion?: string | null;
  }

  interface Props {
    pull: GithubPull;
  }
  let { pull }: Props = $props();

  // The synced PR payload carries an aggregate CI state but not the per-check
  // run list. Read a list defensively if a future sync relays one.
  const runs = $derived(
    ((pull as unknown as { check_runs?: unknown }).check_runs instanceof Array
      ? ((pull as unknown as { check_runs: CheckRun[] }).check_runs)
      : []) satisfies CheckRun[],
  );
  const ci = $derived(ciStateOf(pull));
</script>

{#if runs.length > 0}
  <ul class="checks">
    {#each runs as run (run.name)}
      <li>
        <span class="name">{run.name ?? "check"}</span>
        <span class="state">{run.conclusion ?? run.status ?? "pending"}</span>
      </li>
    {/each}
  </ul>
{:else}
  <PrEmptyTab
    title="Aggregate CI: {ci}"
    detail="Per-check run details are not part of the current sync payload — only the rolled-up CI state above. This tab will list individual checks once the backend relays check runs."
  />
{/if}

<style>
  .checks {
    list-style: none;
    margin: 0;
    padding: var(--gh-space-3) var(--gh-space-4);
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-2);
    overflow: auto;
    height: 100%;
  }
  li {
    display: flex;
    justify-content: space-between;
    gap: var(--gh-space-3);
    font-size: var(--fs-sm);
  }
  .state {
    color: var(--gh-fg-muted);
    font-family: var(--gh-mono);
  }
</style>
