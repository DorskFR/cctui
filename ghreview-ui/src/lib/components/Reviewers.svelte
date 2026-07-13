<script lang="ts" module>
  import type { ReviewerState } from "../api/types";

  const STATE_META: Record<ReviewerState, { icon: string; label: string; cls: string }> = {
    APPROVED: { icon: "✓", label: "Approved", cls: "approved" },
    CHANGES_REQUESTED: { icon: "✗", label: "Changes requested", cls: "changes" },
    COMMENTED: { icon: "💬", label: "Commented", cls: "commented" },
    DISMISSED: { icon: "⊘", label: "Dismissed", cls: "dismissed" },
    PENDING: { icon: "…", label: "Awaiting review", cls: "pending" },
  };
</script>

<script lang="ts">
  import { createQuery } from "@tanstack/svelte-query";
  import { toStore } from "svelte/store";
  import { api } from "../api/client";
  import { keys, queryClient } from "../api/queries";
  import Avatar from "./Avatar.svelte";

  interface Props {
    owner: string;
    repo: string;
    number: number;
    account?: string;
  }
  let { owner, repo, number, account }: Props = $props();

  let pending = $state<string | null>(null);

  const query = createQuery(
    toStore(() => ({
      queryKey: keys.reviewers(owner, repo, number),
      queryFn: () => api.reviewers(owner, repo, number, account as string),
      enabled: account != null,
    })),
  );

  const reviewers = $derived($query.data?.reviewers ?? []);
  const teams = $derived($query.data?.requested_teams ?? []);

  async function reRequest(login: string): Promise<void> {
    if (!account || pending) return;
    pending = login;
    try {
      const result = await api.reRequestReviewers(owner, repo, number, account, [login]);
      queryClient.setQueryData(keys.reviewers(owner, repo, number), result);
    } finally {
      pending = null;
    }
  }
</script>

<section class="reviewers">
  <h2>Reviewers</h2>
  {#if !account}
    <p class="muted">No account.</p>
  {:else if $query.isLoading}
    <p class="muted">Loading reviewers…</p>
  {:else if $query.isError}
    <p class="err">{($query.error as Error).message}</p>
  {:else if reviewers.length === 0 && teams.length === 0}
    <p class="muted">No reviewers requested.</p>
  {:else}
    <ul>
      {#each reviewers as r (r.login)}
        {@const meta = STATE_META[r.state]}
        <li>
          <Avatar user={{ login: r.login, avatar_url: r.avatar_url ?? undefined }} size={20} />
          <span class="login">{r.login}</span>
          <span class="badge {meta.cls}" title={meta.label}>
            <span class="icon" aria-hidden="true">{meta.icon}</span>
            {meta.label}
          </span>
          {#if r.state !== "PENDING"}
            <button
              type="button"
              class="rerequest"
              disabled={pending != null}
              title="Re-request review"
              onclick={() => reRequest(r.login)}
            >
              {pending === r.login ? "…" : "↻"}
            </button>
          {/if}
        </li>
      {/each}
      {#each teams as t (t.slug)}
        <li>
          <span class="teamicon" aria-hidden="true">👥</span>
          <span class="login">{t.name}</span>
          <span class="badge pending">Team</span>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .reviewers {
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-1);
  }
  h2 {
    font-size: var(--fs-xs);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--gh-fg-muted);
    margin: 0;
    font-weight: 600;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-1);
  }
  li {
    display: flex;
    align-items: center;
    gap: var(--gh-space-2);
    font-size: var(--fs-xs);
  }
  .login {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--gh-fg-muted);
  }
  .icon {
    font-weight: 700;
  }
  .badge.approved {
    color: var(--gh-success);
  }
  .badge.changes {
    color: var(--gh-danger);
  }
  .rerequest {
    background: transparent;
    border: 1px solid var(--gh-border);
    color: var(--gh-fg-muted);
    border-radius: var(--gh-radius-sm);
    padding: 0 6px;
    cursor: pointer;
    line-height: 1.4;
  }
  .rerequest:hover:not(:disabled) {
    color: var(--gh-accent);
    border-color: var(--gh-accent);
  }
  .rerequest:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .teamicon {
    width: 20px;
    text-align: center;
  }
  .muted {
    color: var(--gh-fg-muted);
    margin: 0;
    font-size: var(--fs-xs);
  }
  .err {
    color: var(--gh-danger);
    margin: 0;
    font-size: var(--fs-xs);
  }
</style>
