<script lang="ts">
  import type { GithubPull, PrState } from "../../api/types";
  import LabelPicker from "../LabelPicker.svelte";
  import PrStateIcon from "../PrStateIcon.svelte";

  interface Props {
    owner: string;
    repo: string;
    number: number;
    account?: string;
    pull: GithubPull;
    state: PrState;
  }

  let { owner, repo, number, account, pull, state }: Props = $props();
</script>

<div class="identity">
  <span class="state state-{state}">
    <PrStateIcon {state} size={14} inherit />
    {state}
  </span>
  <h1>
    {#if pull.html_url}
      <a class="title-link" href={pull.html_url} target="_blank" rel="noopener noreferrer">
        {pull.title}
      </a>
    {:else}
      {pull.title}
    {/if}
  </h1>
  <span class="number">#{number}</span>
  <div class="labels" aria-label="Pull request labels">
    <LabelPicker {owner} {repo} {number} {account} labels={pull.labels ?? []} />
  </div>
</div>

<style>
  .identity {
    display: flex;
    flex: 1 1 auto;
    min-width: 0;
    align-items: center;
    gap: var(--gh-space-2);
  }
  h1 {
    flex: 0 1 auto;
    min-width: 2.5rem;
    overflow: hidden;
    margin: 0;
    font-size: var(--fs-md);
    font-weight: 600;
    line-height: 1.35;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .title-link {
    color: inherit;
    text-decoration: none;
  }
  .title-link:hover {
    color: var(--gh-accent);
    text-decoration: underline;
  }
  .state {
    display: inline-flex;
    flex: none;
    align-items: center;
    gap: 5px;
    padding: 2px 9px;
    border-radius: 999px;
    font-size: var(--fs-xs);
    font-weight: 600;
    text-transform: capitalize;
  }
  .state-open {
    color: var(--gh-success);
    background: color-mix(in srgb, var(--gh-success) 12%, transparent);
  }
  .state-draft {
    color: var(--gh-draft);
    background: color-mix(in srgb, var(--gh-draft) 12%, transparent);
  }
  .state-merged {
    color: var(--gh-merged);
    background: color-mix(in srgb, var(--gh-merged) 12%, transparent);
  }
  .state-closed {
    color: var(--gh-danger);
    background: color-mix(in srgb, var(--gh-danger) 12%, transparent);
  }
  .number {
    flex: none;
    color: var(--gh-fg-muted);
    font-size: var(--fs-sm);
  }
  .labels {
    flex: none;
  }

  @media (max-width: 700px) {
    .identity {
      display: grid;
      grid-template-columns: auto auto 1fr;
      order: 0;
    }
    h1 {
      grid-column: 1 / -1;
      grid-row: 2;
      overflow: visible;
      min-width: 0;
      text-overflow: clip;
      white-space: normal;
    }
    .labels {
      justify-self: end;
      min-width: 0;
    }
  }
</style>
