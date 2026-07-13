<script lang="ts">
  import type { GithubPull } from "../api/types";
  import PrEmptyTab from "./PrEmptyTab.svelte";

  interface CommitEntry {
    sha?: string;
    commit?: {
      message?: string;
      author?: { name?: string; date?: string } | null;
    };
    author?: { login?: string } | null;
  }

  interface Props {
    pull: GithubPull;
    owner?: string;
    repo?: string;
  }
  let { pull, owner, repo }: Props = $props();

  const commits = $derived(
    ((pull as unknown as { commits_list?: unknown }).commits_list instanceof Array
      ? (pull as unknown as { commits_list: CommitEntry[] }).commits_list
      : []) satisfies CommitEntry[],
  );

  function shortSha(sha: string | undefined): string {
    return sha ? sha.slice(0, 7) : "";
  }

  function subject(entry: CommitEntry): string {
    return (entry.commit?.message ?? "").split("\n")[0];
  }

  function authored(entry: CommitEntry): string {
    const d = entry.commit?.author?.date;
    return d ? new Date(d).toLocaleDateString() : "";
  }

  function commitUrl(sha: string | undefined): string | null {
    return owner && repo && sha ? `https://github.com/${owner}/${repo}/commit/${sha}` : null;
  }
</script>

{#if commits.length > 0}
  <ul class="commits">
    {#each commits as c (c.sha)}
      <li>
        {#if commitUrl(c.sha)}
          <a class="sha" href={commitUrl(c.sha)} target="_blank" rel="noopener noreferrer">
            {shortSha(c.sha)}
          </a>
        {:else}
          <code class="sha">{shortSha(c.sha)}</code>
        {/if}
        <span class="subject">{subject(c)}</span>
        <span class="author">{c.author?.login ?? c.commit?.author?.name ?? ""}</span>
        {#if authored(c)}<span class="date">{authored(c)}</span>{/if}
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
    font-size: var(--fs-sm);
  }
  .sha {
    font-family: var(--gh-mono);
    background: var(--gh-bg-inset);
    padding: 0 6px;
    border-radius: var(--gh-radius-sm);
    text-decoration: none;
    color: var(--gh-accent);
  }
  a.sha:hover {
    text-decoration: underline;
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
  .date {
    color: var(--gh-fg-muted);
    font-variant-numeric: tabular-nums;
  }
</style>
