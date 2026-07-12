<script lang="ts">
  import type { CommentAnchor } from "../review/anchors";
  import InlineCommentComposer from "./InlineCommentComposer.svelte";

  interface Props {
    anchor: CommentAnchor;
    onAdd: (body: string) => void;
    onEdit: (id: string, body: string) => void;
    onDelete: (id: string) => void;
    onClose: () => void;
    pending?: boolean;
  }
  let { anchor, onAdd, onEdit, onDelete, onClose, pending = false }: Props = $props();

  let editing = $state<string | null>(null);
  let replying = $state(false);

  const isEmpty = $derived(anchor.drafts.length === 0 && anchor.published.length === 0);

  $effect(() => {
    if (isEmpty) replying = true;
  });
</script>

<div class="thread">
  <div class="thead">
    <span class="loc">{anchor.path}:{anchor.line} · {anchor.side}</span>
    <button type="button" class="x" aria-label="Close" onclick={onClose}>×</button>
  </div>

  {#each anchor.published as c (c.id)}
    <div class="comment published">
      <div class="meta">{c.user ?? "someone"} · published</div>
      <div class="body">{c.body}</div>
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
    font-size: 11px;
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
    font-size: 11px;
    color: var(--gh-fg-muted);
    margin-bottom: 2px;
  }
  .ctrls {
    display: flex;
    gap: var(--gh-space-2);
  }
  .body {
    font-size: 12px;
    white-space: pre-wrap;
    word-break: break-word;
  }
  button {
    font-size: 11px;
    background: none;
    border: none;
    color: var(--gh-accent);
    cursor: pointer;
    padding: 0;
  }
  .x {
    color: var(--gh-fg-muted);
    font-size: 14px;
  }
  .reply {
    align-self: flex-start;
  }
</style>
