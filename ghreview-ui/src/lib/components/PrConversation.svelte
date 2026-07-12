<script lang="ts">
  import type { GithubPull } from "../api/types";
  import PrEmptyTab from "./PrEmptyTab.svelte";

  interface TimelineComment {
    id?: number | string;
    user?: { login?: string } | null;
    body?: string | null;
    created_at?: string;
  }

  interface Props {
    pull: GithubPull;
  }
  let { pull }: Props = $props();

  // The synced PR payload does not currently include the issue-comment or
  // review-thread timeline, so this reads them defensively if a future sync
  // starts relaying them, and otherwise shows a placeholder.
  const comments = $derived(
    ((pull as unknown as { comments?: unknown }).comments instanceof Array
      ? ((pull as unknown as { comments: TimelineComment[] }).comments)
      : []) satisfies TimelineComment[],
  );
</script>

{#if comments.length > 0}
  <ul class="timeline">
    {#each comments as c (c.id ?? `${c.user?.login}-${c.created_at}`)}
      <li>
        <div class="meta">{c.user?.login ?? "unknown"}</div>
        <div class="body">{(c.body ?? "").trim()}</div>
      </li>
    {/each}
  </ul>
{:else}
  <PrEmptyTab
    title="No conversation synced"
    detail="Issue comments and review threads are not part of the current sync payload. This tab will populate once the backend relays the PR timeline."
  />
{/if}

<style>
  .timeline {
    list-style: none;
    margin: 0;
    padding: var(--gh-space-4);
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-3);
    overflow: auto;
    height: 100%;
  }
  .meta {
    font-weight: 600;
    font-size: 13px;
  }
  .body {
    white-space: pre-wrap;
    word-break: break-word;
    font-size: 14px;
    line-height: 1.5;
    margin-top: var(--gh-space-1);
  }
</style>
