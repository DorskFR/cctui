<script lang="ts">
  import { api, ApiError } from "../api/client";
  import type { GithubPull, ReactionContent, ReactionRollup } from "../api/types";
  import { renderMarkdown } from "../markdown";
  import PrEmptyTab from "./PrEmptyTab.svelte";
  import ReactionBar from "./ReactionBar.svelte";

  interface TimelineComment {
    id?: number | string;
    user?: { login?: string } | null;
    body?: string | null;
    created_at?: string;
    reactions?: ReactionRollup | null;
  }

  interface Props {
    pull: GithubPull;
    owner?: string;
    repo?: string;
    account?: string;
  }
  let { pull, owner, repo, account }: Props = $props();

  // The synced PR payload does not currently include the issue-comment or
  // review-thread timeline, so this reads them defensively if a future sync
  // starts relaying them, and otherwise shows a placeholder.
  const comments = $derived(
    ((pull as unknown as { comments?: unknown }).comments instanceof Array
      ? ((pull as unknown as { comments: TimelineComment[] }).comments)
      : []) satisfies TimelineComment[],
  );

  const canReact = $derived(
    owner !== undefined && repo !== undefined && account !== undefined,
  );

  let confirmingDelete = $state<number | null>(null);
  let deleting = $state<number | null>(null);
  let deletedIds = $state<Set<number>>(new Set());
  let deleteError = $state<string | null>(null);

  const visibleComments = $derived(
    comments.filter((c) => {
      const id = commentId(c);
      return id === null || !deletedIds.has(id);
    }),
  );

  function commentId(c: TimelineComment): number | null {
    return typeof c.id === "number" ? c.id : null;
  }

  function ownsComment(c: TimelineComment): boolean {
    return account !== undefined && !!c.user?.login && c.user.login === account;
  }

  async function deleteComment(id: number): Promise<void> {
    if (owner === undefined || repo === undefined || account === undefined) return;
    deleting = id;
    deleteError = null;
    try {
      await api.deleteIssueComment(owner, repo, id, account);
      deletedIds = new Set(deletedIds).add(id);
      confirmingDelete = null;
    } catch (e) {
      deleteError = e instanceof ApiError ? e.message : "Failed to delete comment";
    } finally {
      deleting = null;
    }
  }
</script>

{#if visibleComments.length > 0}
  <ul class="timeline">
    {#each visibleComments as c (c.id ?? `${c.user?.login}-${c.created_at}`)}
      <li>
        <div class="meta">
          <span>{c.user?.login ?? "unknown"}</span>
          {#if ownsComment(c) && commentId(c) !== null}
            <span class="ctrls">
              {#if confirmingDelete === commentId(c)}
                <button
                  type="button"
                  class="danger"
                  disabled={deleting === commentId(c)}
                  onclick={() => deleteComment(commentId(c) as number)}
                >{deleting === commentId(c) ? "Deleting…" : "Confirm"}</button>
                <button type="button" onclick={() => (confirmingDelete = null)}>Cancel</button>
              {:else}
                <button
                  type="button"
                  class="danger"
                  aria-label="Delete comment"
                  onclick={() => {
                    deleteError = null;
                    confirmingDelete = commentId(c);
                  }}
                >Delete</button>
              {/if}
            </span>
          {/if}
        </div>
        <div class="body markdown">{@html renderMarkdown((c.body ?? "").trim())}</div>
        {#if deleteError && confirmingDelete === commentId(c)}
          <div class="err">{deleteError}</div>
        {/if}
        {#if canReact && commentId(c) !== null}
          <ReactionBar
            reactions={c.reactions ?? null}
            onToggle={(content: ReactionContent) =>
              api.toggleIssueCommentReaction(
                owner as string,
                repo as string,
                commentId(c) as number,
                account as string,
                content,
              )}
          />
        {/if}
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
    display: flex;
    justify-content: space-between;
    align-items: center;
    font-weight: 600;
    font-size: var(--fs-sm);
  }
  .ctrls {
    display: flex;
    gap: var(--gh-space-2);
    font-weight: 400;
  }
  .ctrls button {
    font-size: var(--fs-xs);
    background: none;
    border: none;
    color: var(--gh-accent);
    cursor: pointer;
    padding: 0;
  }
  .ctrls button.danger {
    color: var(--danger, #f85149);
  }
  .err {
    font-size: var(--fs-xs);
    color: var(--danger, #f85149);
    margin-top: var(--gh-space-1);
  }
  .body {
    margin-top: var(--gh-space-1);
  }
</style>
