<script lang="ts" module>
  import type { ActivityEvent, ReactionRollup, ReviewThreadComment } from "../api/types";

  export interface CommentEntry {
    key: string;
    kind: "issue" | "review" | "inline";
    id: number | string | null;
    author: string;
    avatarUrl: string | null;
    body: string;
    createdAt: string | null;
    htmlUrl: string | null;
    reviewState: string | null;
    reactions: ReactionRollup | null;
  }

  export interface CommentGroup {
    key: string;
    entries: CommentEntry[];
  }

  export type CommentViewState = "no-account" | "loading" | "error" | "empty" | "content";

  export function commentViewState(input: {
    account?: string;
    loading: boolean;
    error: Error | null;
    groups: CommentGroup[];
  }): CommentViewState {
    if (!input.account) return "no-account";
    if (input.loading && input.groups.length === 0) return "loading";
    if (input.error && input.groups.length === 0) return "error";
    return input.groups.length === 0 ? "empty" : "content";
  }

  function timestamp(value: string | null): number {
    if (!value) return 0;
    const parsed = Date.parse(value);
    return Number.isNaN(parsed) ? 0 : parsed;
  }

  export function buildCommentGroups(
    activity: ActivityEvent[],
    inline: ReviewThreadComment[],
  ): CommentGroup[] {
    const groups: CommentGroup[] = activity
      .filter((event) => event.event === "commented" || event.event === "reviewed")
      .map((event, index) => ({
        key: `activity-${event.id ?? `${event.event}-${event.created_at}-${index}`}`,
        entries: [
          {
            key: `activity-${event.id ?? index}`,
            kind: event.event === "reviewed" ? "review" : "issue",
            id: event.id,
            author: event.actor?.login ?? "unknown",
            avatarUrl: event.actor?.avatar_url ?? null,
            body: event.detail?.body ?? "",
            createdAt: event.created_at,
            htmlUrl: event.html_url,
            reviewState: event.detail?.state ?? null,
            reactions: event.reactions ?? null,
          },
        ],
      }));

    const byRoot = new Map<number, CommentGroup>();
    for (const comment of inline) {
      const rootId = comment.in_reply_to_id ?? comment.id;
      let group = byRoot.get(rootId);
      if (!group) {
        group = { key: `inline-${rootId}`, entries: [] };
        byRoot.set(rootId, group);
        groups.push(group);
      }
      group.entries.push({
        key: `inline-${comment.id}`,
        kind: "inline",
        id: comment.id,
        author: comment.user ?? "unknown",
        avatarUrl: null,
        body: comment.body,
        createdAt: comment.created_at,
        htmlUrl: comment.html_url,
        reviewState: null,
        reactions: comment.reactions,
      });
    }

    for (const group of groups) {
      group.entries.sort((a, b) => timestamp(a.createdAt) - timestamp(b.createdAt));
    }
    return groups.sort(
      (a, b) => timestamp(a.entries[0]?.createdAt ?? null) - timestamp(b.entries[0]?.createdAt ?? null),
    );
  }

  export function reviewLabel(state: string | null): string {
    if (state === "APPROVED") return "Approved";
    if (state === "CHANGES_REQUESTED") return "Requested changes";
    if (state === "DISMISSED") return "Dismissed review";
    return "Reviewed";
  }
</script>

<script lang="ts">
  import { createQuery } from "@tanstack/svelte-query";
  import { toStore } from "svelte/store";
  import { api } from "../api/client";
  import { keys } from "../api/queries";
  import type { ReactionContent, ReactionSummary } from "../api/types";
  import { renderMarkdown } from "../markdown";
  import Avatar from "./Avatar.svelte";
  import ReactionBar from "./ReactionBar.svelte";

  interface Props {
    owner: string;
    repo: string;
    number: number;
    account?: string;
    inline: ReviewThreadComment[];
    inlineLoading?: boolean;
    inlineError?: Error | null;
  }
  let {
    owner,
    repo,
    number,
    account,
    inline,
    inlineLoading = false,
    inlineError = null,
  }: Props = $props();

  const activityQuery = createQuery(
    toStore(() => ({
      queryKey: keys.activity(owner, repo, number, account),
      queryFn: () => api.activity(owner, repo, number, account as string),
      enabled: account != null,
    })),
  );
  const groups = $derived(buildCommentGroups($activityQuery.data?.items ?? [], inline));
  const loading = $derived($activityQuery.isLoading || inlineLoading);
  const error = $derived(($activityQuery.error as Error | null) ?? inlineError);
  const viewState = $derived(commentViewState({ account, loading, error, groups }));

  function numericId(entry: CommentEntry): number | null {
    const id = Number(entry.id);
    return Number.isInteger(id) && id > 0 ? id : null;
  }

  async function toggleReaction(
    entry: CommentEntry,
    content: ReactionContent,
  ): Promise<ReactionSummary> {
    const id = numericId(entry);
    if (!account || id === null) throw new Error("Comment reactions are unavailable");
    return entry.kind === "inline"
      ? api.toggleReviewCommentReaction(owner, repo, id, account, content)
      : api.toggleIssueCommentReaction(owner, repo, id, account, content);
  }
</script>

<div class="comments">
  {#if viewState === "no-account"}
    <p class="muted">No account is available for comments.</p>
  {:else if viewState === "loading"}
    <p class="muted">Loading comments…</p>
  {:else if viewState === "error"}
    <p class="err">{error?.message}</p>
  {:else if viewState === "empty"}
    <p class="muted">No comments yet.</p>
  {:else}
    {#if error}<p class="err">Some comments could not be loaded: {error.message}</p>{/if}
    <ol class="comment-list">
      {#each groups as group (group.key)}
        <li class="group">
          {#each group.entries as entry (entry.key)}
            <article class:reply={group.entries.length > 1 && entry !== group.entries[0]}>
              <header>
                <Avatar
                  user={{ login: entry.author, avatar_url: entry.avatarUrl ?? undefined }}
                  size={24}
                />
                <strong>{entry.author}</strong>
                {#if entry.kind === "review"}
                  <span class="review-state">{reviewLabel(entry.reviewState)}</span>
                {:else if entry.kind === "inline"}
                  <span class="kind">review comment</span>
                {/if}
                {#if entry.createdAt}
                  <time datetime={entry.createdAt}>{new Date(entry.createdAt).toLocaleString()}</time>
                {/if}
                {#if entry.htmlUrl}
                  <a href={entry.htmlUrl} target="_blank" rel="noopener noreferrer">View on GitHub</a>
                {/if}
              </header>
              {#if entry.body}
                <div class="body markdown">{@html renderMarkdown(entry.body)}</div>
              {/if}
              {#if entry.kind !== "review" && numericId(entry) !== null}
                <ReactionBar
                  reactions={entry.reactions}
                  onToggle={(content) => toggleReaction(entry, content)}
                />
              {/if}
            </article>
          {/each}
        </li>
      {/each}
    </ol>
  {/if}
</div>

<style>
  .comments {
    height: 100%;
    overflow: auto;
    padding: var(--gh-space-4);
  }
  .comment-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-3);
  }
  .group {
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
    overflow: hidden;
  }
  article {
    padding: var(--gh-space-3);
  }
  article.reply {
    border-top: 1px solid var(--gh-border);
    padding-left: var(--gh-space-4);
  }
  header {
    display: flex;
    align-items: center;
    gap: var(--gh-space-2);
    min-width: 0;
    font-size: var(--fs-sm);
  }
  time,
  .kind {
    color: var(--gh-fg-muted);
  }
  time {
    margin-left: auto;
  }
  header a {
    color: var(--gh-accent);
    text-decoration: none;
  }
  .review-state {
    color: var(--gh-fg-muted);
    border: 1px solid var(--gh-border);
    border-radius: 999px;
    padding: 0 var(--gh-space-2);
  }
  .body {
    margin-top: var(--gh-space-2);
  }
  .muted {
    color: var(--gh-fg-muted);
    margin: 0;
  }
  .err {
    color: var(--gh-danger);
    margin: 0 0 var(--gh-space-2);
  }
</style>
