<script lang="ts">
  import { createQuery, useQueryClient } from "@tanstack/svelte-query";
  import { Button, SegmentedControl, Select } from "@dorsk/tsumikit";
  import { toStore } from "svelte/store";
  import { api, type NotificationFilter } from "../api/client";
  import { getAccount } from "../api/config";
  import {
    type GithubNotification,
    type NotificationInboxItem,
    notificationOf,
  } from "../api/types";
  import { parsePullApiUrl, pullPath } from "../router/route";
  import { router } from "../router/router.svelte";
  import { tabs } from "../stores/tabs.svelte";
  import RepoBadge from "./RepoBadge.svelte";

  const account = getAccount() ?? "";
  const client = useQueryClient();

  let reason = $state<string>("");
  let repoFilter = $state<string>("");
  let status = $state<string>("unread");
  let selected = $state<Set<string>>(new Set());

  const statusOptions = [
    { value: "unread", label: "Unread" },
    { value: "read", label: "Read" },
    { value: "done", label: "Done" },
    { value: "archived", label: "Archived" },
    { value: "all", label: "All" },
  ];

  const filter = $derived<NotificationFilter>({
    account: account || undefined,
    reason: reason || undefined,
    all: "true",
  });

  const query = createQuery(
    toStore(() => ({
      queryKey: ["notifications", JSON.stringify(filter)],
      queryFn: () => api.notifications(filter),
    })),
  );

  const items = $derived<NotificationInboxItem[]>($query.data?.items ?? []);

  function repoName(item: NotificationInboxItem): string {
    return notificationOf(item).repository?.full_name ?? "";
  }

  function matchesStatus(item: NotificationInboxItem): boolean {
    const s = item.state;
    const unread = notificationOf(item).unread && !s.read;
    switch (status) {
      case "unread":
        return unread && !s.archived;
      case "read":
        return !unread && !s.done && !s.archived;
      case "done":
        return s.done && !s.archived;
      case "archived":
        return s.archived;
      default:
        return true;
    }
  }

  const repos = $derived(
    [...new Set(items.map(repoName).filter(Boolean))].sort((a, b) => a.localeCompare(b)),
  );

  const repoCounts = $derived.by(() => {
    const counts = new Map<string, number>();
    for (const item of items) {
      if (!matchesStatus(item)) continue;
      const r = repoName(item);
      if (r) counts.set(r, (counts.get(r) ?? 0) + 1);
    }
    return [...counts.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
  });

  const visible = $derived(
    items.filter(
      (item) => matchesStatus(item) && (!repoFilter || repoName(item) === repoFilter),
    ),
  );

  function toggle(id: string): void {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    selected = next;
  }

  function isPull(n: GithubNotification): boolean {
    return n.subject.type === "PullRequest" && parsePullApiUrl(n.subject.url) !== null;
  }

  function openPull(n: GithubNotification): void {
    const ref = parsePullApiUrl(n.subject.url);
    if (!ref) return;
    tabs.open(ref.owner, ref.repo, ref.number, n.subject.title);
    router.navigate(pullPath(ref.owner, ref.repo, ref.number));
  }

  const markDonePatch = { read: true, done: true, archived: true };

  async function markDone(ids: string[]): Promise<void> {
    if (ids.length === 0 || !account) return;
    await api.setNotificationState(account, ids, markDonePatch);
    selected = new Set([...selected].filter((id) => !ids.includes(id)));
    client.invalidateQueries({ queryKey: ["notifications"] });
  }
</script>

<div class="wrap">
  <div class="toolbar">
    <SegmentedControl options={statusOptions} bind:value={status} size="sm" label="Status" />
    <Select compact bind:value={reason} aria-label="Reason">
      <option value="">All reasons</option>
      <option value="review_requested">Review requested</option>
      <option value="mention">Mention</option>
      <option value="ci_activity">CI activity</option>
    </Select>
    <Select compact bind:value={repoFilter} aria-label="Repository">
      <option value="">All repos</option>
      {#each repos as r (r)}
        <option value={r}>{r}</option>
      {/each}
    </Select>
    <div class="spacer"></div>
    <Button size="sm" disabled={selected.size === 0} onclick={() => markDone([...selected])}>
      Mark done{selected.size ? ` (${selected.size})` : ""}
    </Button>
  </div>

  {#if repoCounts.length > 0}
    <div class="badges">
      {#each repoCounts as [repo, count] (repo)}
        <button
          type="button"
          class="badge-btn"
          class:active={repoFilter === repo}
          onclick={() => (repoFilter = repoFilter === repo ? "" : repo)}
        >
          <RepoBadge {repo} {count} />
        </button>
      {/each}
    </div>
  {/if}

  {#if $query.isLoading}
    <div class="msg">Loading…</div>
  {:else if $query.isError}
    <div class="msg err">{($query.error as Error).message}</div>
  {:else if visible.length === 0}
    <div class="msg">Inbox zero.</div>
  {:else}
    <ul class="list">
      {#each visible as item (notificationOf(item).id || item.synced_at)}
        {@const n = notificationOf(item)}
        <li class:unread={n.unread && !item.state.read}>
          <input
            type="checkbox"
            checked={selected.has(n.id)}
            onchange={() => toggle(n.id)}
            onclick={(e) => e.stopPropagation()}
          />
          {#if isPull(n)}
            <button type="button" class="body open" onclick={() => openPull(n)}>
              <span class="subject">{n.subject.title}</span>
              <div class="sub">
                {#if n.repository?.full_name}
                  <RepoBadge repo={n.repository.full_name} />
                {/if}
                <span class="reason">{n.reason}</span>
              </div>
            </button>
          {:else}
            <div class="body">
              <span class="subject">{n.subject.title}</span>
              <div class="sub">
                {#if n.repository?.full_name}
                  <RepoBadge repo={n.repository.full_name} />
                {/if}
                <span class="reason">{n.reason}</span>
              </div>
            </div>
          {/if}
          <div class="actions">
            <Button variant="ghost" size="sm" onclick={() => markDone([n.id])}>Mark done</Button>
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .wrap {
    padding: var(--gh-space-3);
  }
  .toolbar {
    display: flex;
    align-items: center;
    gap: var(--gh-space-2);
    margin-bottom: var(--gh-space-3);
  }
  .spacer {
    flex: 1;
  }
  .badges {
    display: flex;
    flex-wrap: wrap;
    gap: var(--gh-space-2);
    margin-bottom: var(--gh-space-3);
  }
  .badge-btn {
    padding: 0;
    background: none;
    border: none;
    border-radius: var(--gh-radius-sm);
    max-width: 240px;
  }
  .badge-btn.active {
    outline: 1px solid var(--gh-accent);
    outline-offset: 1px;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
  }
  li {
    display: flex;
    align-items: center;
    gap: var(--gh-space-3);
    padding: var(--gh-space-2) var(--gh-space-3);
    border-bottom: 1px solid var(--gh-border-muted);
  }
  li.unread {
    box-shadow: inset 2px 0 0 var(--gh-accent);
  }
  .body {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }
  button.body {
    background: none;
    border: none;
    padding: 0;
    color: var(--gh-fg);
    align-items: flex-start;
    text-align: left;
    cursor: pointer;
  }
  button.body:hover .subject {
    text-decoration: underline;
  }
  .subject {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sub {
    display: flex;
    align-items: center;
    gap: var(--gh-space-2);
    min-width: 0;
    color: var(--gh-fg-muted);
    font-size: 12px;
  }
  .reason {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .actions {
    display: flex;
    gap: 4px;
  }
  .msg {
    padding: var(--gh-space-4);
    color: var(--gh-fg-muted);
  }
  .err {
    color: var(--gh-danger);
  }
</style>
