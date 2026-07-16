<script lang="ts" module>
  import type { IconName } from "@dorsk/tsumikit";
  import type { ReviewerState } from "../api/types";

  type BadgeTone = "neutral" | "ok" | "warn" | "danger" | "info";

  const STATE_META: Record<ReviewerState, { icon: IconName; label: string; tone: BadgeTone }> = {
    APPROVED: { icon: "check-circle", label: "Approved", tone: "ok" },
    CHANGES_REQUESTED: { icon: "x-circle", label: "Changes requested", tone: "danger" },
    COMMENTED: { icon: "info", label: "Commented", tone: "info" },
    DISMISSED: { icon: "minus", label: "Dismissed", tone: "neutral" },
    PENDING: { icon: "clock", label: "Awaiting review", tone: "neutral" },
  };
</script>

<script lang="ts">
  import { Badge, Button, Icon, IconButton, Input, Popover } from "@dorsk/tsumikit";
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
  let addLogin = $state("");

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

  async function addReviewer(): Promise<void> {
    const login = addLogin.trim();
    if (!account || pending || !login) return;
    await reRequest(login);
    addLogin = "";
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
  {:else}
    {#if reviewers.length === 0 && teams.length === 0}
      <p class="muted">No reviewers requested.</p>
    {:else}
      <ul>
        {#each reviewers as r (r.login)}
          {@const meta = STATE_META[r.state]}
          {@const requested = r.requested ?? false}
          <li>
            <Avatar user={{ login: r.login, avatar_url: r.avatar_url ?? undefined }} size={20} />
            <span class="login">{r.login}</span>
            <Badge tone={meta.tone} size="sm">
              <Icon name={meta.icon} label={meta.label} />
              {requested && r.state === "PENDING" ? "Requested" : meta.label}
            </Badge>
            {#if r.state !== "PENDING"}
              <IconButton
                icon="retry"
                label="Re-request review from {r.login}"
                size={16}
                disabled={pending != null}
                onclick={() => reRequest(r.login)}
              />
            {/if}
          </li>
        {/each}
        {#each teams as t (t.slug)}
          <li>
            <Icon name="users" label="Team" />
            <span class="login">{t.name}</span>
            <Badge tone="neutral" size="sm">Team</Badge>
          </li>
        {/each}
      </ul>
    {/if}
    <Popover label="Request a reviewer" placement="bottom-start" size="sm">
      {#snippet trigger()}<Icon name="plus" /> Add{/snippet}
      <div class="add-panel">
        <Input
          bind:value={addLogin}
          placeholder="GitHub login"
          size="sm"
          disabled={pending != null}
          onkeydown={(e: KeyboardEvent) => {
            if (e.key === "Enter") addReviewer();
          }}
        />
        <Button
          size="sm"
          variant="primary"
          tone="accent"
          disabled={pending != null || !addLogin.trim()}
          onclick={addReviewer}
        >
          Request
        </Button>
      </div>
    </Popover>
  {/if}
</section>

<style>
  .reviewers {
    display: flex;
    min-width: max-content;
    align-items: center;
    gap: var(--gh-space-2);
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
    align-items: center;
    gap: var(--gh-space-2);
  }
  li {
    display: flex;
    align-items: center;
    gap: var(--gh-space-2);
    font-size: var(--fs-xs);
    padding: 4px 8px 4px 5px;
    border: 1px solid var(--gh-border);
    border-radius: 999px;
    background: var(--gh-bg-inset);
  }
  .login {
    max-width: 12rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .add-panel {
    display: flex;
    align-items: center;
    gap: var(--gh-space-2);
    padding: var(--gh-space-2);
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
