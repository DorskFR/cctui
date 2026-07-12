<script lang="ts">
  import type { GithubPull } from "../api/types";
  import PrEmptyTab from "./PrEmptyTab.svelte";

  interface CommitEntry {
    sha?: string;
    commit?: { message?: string; author?: { name?: string } | null };
    author?: { login?: string } | null;
  }

  interface Props {
    pull: GithubPull;
  }
  let { pull }: Props = $props();

  // The synced PR payload exposes a `commits` count but not the commit list.
  // Read a list defensively in case a future sync relays one, otherwise render
  // a placeholder.
  const commits = $derived(
    ((pull as unknown as { commits?: unknown }).commits instanceof Array
      ? ((pull as unknown as { commits: CommitEntry[] }).commits)
      : []) satisfies CommitEntry[],
  );

  function shortSha(sha: string | undefined): string {
    return sha ? sha.slice(0, 7) : "";
  }

  function subject(entry: CommitEntry): string {
    return (entry.commit?.message ?? "").split("\n")[0];
  }
</script>

{#if commits.length > 0}
  <ul class="commits">
    {#each commits as c (c.sha)}
      <li>
        <code>{shortSha(c.sha)}</code>
        <span class="subject">{subject(c)}</span>
        <span class="author">{c.author?.login ?? c.commit?.author?.name ?? ""}</span>
      </li>
    {/each}
  </ul>
{:else}
  <PrEmptyTab
    title="No commit list synced"
    detail="The current sync payload records a commit count but not the individual commits. This tab will list them once the backend relays the commit list."
  />
{/if}

<style>
  .commits {
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
    align-items: baseline;
    gap: var(--gh-space-3);
    font-size: 13px;
  }
  code {
    font-family: var(--gh-mono);
    background: var(--gh-bg-inset);
    padding: 0 6px;
    border-radius: var(--gh-radius-sm);
  }
  .subject {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .author {
    color: var(--gh-fg-muted);
  }
</style>
