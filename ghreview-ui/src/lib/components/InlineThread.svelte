<script lang="ts">
  import { api, ApiError } from "../api/client";
  import { queryClient } from "../api/queries";
  import type { ReactionContent } from "../api/types";
  import { renderMarkdown } from "../markdown";
  import type { CommentAnchor } from "../review/anchors";
  import InlineCommentComposer from "./InlineCommentComposer.svelte";
  import ReactionBar from "./ReactionBar.svelte";

  interface Props {
    anchor: CommentAnchor;
    onAdd: (body: string) => void;
    onEdit: (id: string, body: string) => void;
    onDelete: (id: string) => void;
    onClose: () => void;
    pending?: boolean;
    owner?: string;
    repo?: string;
    account?: string;
  }
  let {
    anchor,
    onAdd,
    onEdit,
    onDelete,
    onClose,
    pending = false,
    owner,
    repo,
    account,
  }: Props = $props();

  const canReact = $derived(owner !== undefined && repo !== undefined && account !== undefined);

  let editing = $state<string | null>(null);
  let replying = $state(false);

  let confirmingDelete = $state<number | null>(null);
  let deleting = $state<number | null>(null);
  let deletedIds = $state<Set<number>>(new Set());
  let deleteError = $state<string | null>(null);

  const visiblePublished = $derived(anchor.published.filter((c) => !deletedIds.has(c.id)));
  const isEmpty = $derived(anchor.drafts.length === 0 && visiblePublished.length === 0);

  function ownsPublished(user: string | null): boolean {
    return account !== undefined && user !== null && user === account;
  }

  async function deletePublished(id: number): Promise<void> {
    if (owner === undefined || repo === undefined || account === undefined) return;
    deleting = id;
    deleteError = null;
    try {
      await api.deletePublishedReviewComment(owner, repo, id, account);
      deletedIds = new Set(deletedIds).add(id);
      confirmingDelete = null;
      queryClient.invalidateQueries({ queryKey: ["review-threads", owner, repo] });
    } catch (e) {
      deleteError = e instanceof ApiError ? e.message : "Failed to delete comment";
    } finally {
      deleting = null;
    }
  }

  $effect(() => {
    if (isEmpty) replying = true;
  });
</script>

<div class="thread">
  <div class="thead">
    <span class="loc">{anchor.path}:{anchor.line} · {anchor.side}</span>
    <button type="button" class="x" aria-label="Close" onclick={onClose}>×</button>
  </div>

  {#each visiblePublished as c (c.id)}
    <div class="comment published">
      <div class="meta">
        <span>{c.user ?? "someone"} · published</span>
        {#if ownsPublished(c.user)}
          <span class="ctrls">
            {#if confirmingDelete === c.id}
              <button
                type="button"
                class="danger"
                disabled={deleting === c.id}
                onclick={() => deletePublished(c.id)}
              >{deleting === c.id ? "Deleting…" : "Confirm"}</button>
              <button type="button" onclick={() => (confirmingDelete = null)}>Cancel</button>
            {:else}
              <button
                type="button"
                class="danger"
                aria-label="Delete comment"
                onclick={() => {
                  deleteError = null;
                  confirmingDelete = c.id;
                }}
              >Delete</button>
            {/if}
          </span>
        {/if}
      </div>
      <div class="body markdown">{@html renderMarkdown(c.body ?? "")}</div>
      {#if deleteError && confirmingDelete === c.id}
        <div class="err">{deleteError}</div>
      {/if}
      {#if canReact}
        <ReactionBar
          reactions={c.reactions ?? null}
          onToggle={(content: ReactionContent) =>
            api.toggleReviewCommentReaction(
              owner as string,
              repo as string,
              c.id,
              account as string,
              content,
            )}
        />
      {/if}
    </div>
  {/each}

  {#each anchor.drafts as c (c.id)}
    <div class="comment draft">
      <div class="meta">
        <span>draft</span>
        <span class="ctrls">
          <button type="button" onclick={() => (editing = editing === c.id ? null : c.id)}>
            {editing === c.id ? "Cancel" : "Edit"}
          </button>
          <button type="button" onclick={() => onDelete(c.id)}>Delete</button>
        </span>
      </div>
      {#if editing === c.id}
        <InlineCommentComposer
          initial={c.body}
          submitLabel="Save"
          {pending}
          onsubmit={(body) => {
            onEdit(c.id, body);
            editing = null;
          }}
          oncancel={() => (editing = null)}
        />
      {:else}
        <div class="body">{c.body}</div>
      {/if}
    </div>
  {/each}

  {#if replying}
    <InlineCommentComposer
      {pending}
      submitLabel={isEmpty ? "Add comment" : "Add another"}
      onsubmit={(body) => {
        onAdd(body);
        replying = false;
      }}
      oncancel={() => (isEmpty ? onClose() : (replying = false))}
    />
  {:else}
    <button type="button" class="reply" onclick={() => (replying = true)}>Add comment</button>
  {/if}
</div>

<style>
  .thread {
    background: var(--gh-bg-elev);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
    padding: var(--gh-space-2);
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.4);
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-2);
    font-family: var(--gh-font);
  }
  .thead {
    display: flex;
    justify-content: space-between;
    align-items: center;
    font-size: var(--fs-xs);
    color: var(--gh-fg-muted);
  }
  .loc {
    font-family: var(--gh-mono);
  }
  .comment {
    border-top: 1px solid var(--gh-border);
    padding-top: var(--gh-space-1);
  }
  .comment.draft {
    border-left: 2px solid var(--gh-accent);
    padding-left: var(--gh-space-2);
  }
  .meta {
    display: flex;
    justify-content: space-between;
    font-size: var(--fs-xs);
    color: var(--gh-fg-muted);
    margin-bottom: 2px;
  }
  .ctrls {
    display: flex;
    gap: var(--gh-space-2);
  }
  .body {
    font-size: var(--fs-xs);
    white-space: pre-wrap;
    word-break: break-word;
  }
  button {
    font-size: var(--fs-xs);
    background: none;
    border: none;
    color: var(--gh-accent);
    cursor: pointer;
    padding: 0;
  }
  .x {
    color: var(--gh-fg-muted);
    font-size: var(--fs-base);
  }
  button.danger {
    color: var(--danger, #f85149);
  }
  .err {
    font-size: var(--fs-xs);
    color: var(--danger, #f85149);
    margin-top: 2px;
  }
  .reply {
    align-self: flex-start;
  }
</style>
