<script lang="ts">
  import { createQuery, useQueryClient } from "@tanstack/svelte-query";
  import { toStore } from "svelte/store";
  import { api, type NotificationFilter } from "../api/client";
  import { getAccount } from "../api/config";
  import { notificationOf } from "../api/types";

  const account = getAccount() ?? "";
  const client = useQueryClient();

  let reason = $state<string>("");
  let showArchived = $state(false);
  let selected = $state<Set<string>>(new Set());

  const filter = $derived<NotificationFilter>({
    account: account || undefined,
    reason: reason || undefined,
    undone: showArchived ? undefined : "true",
    archived: showArchived ? "true" : undefined,
  });

  const query = createQuery(
    toStore(() => ({
      queryKey: ["notifications", JSON.stringify(filter)],
      queryFn: () => api.notifications(filter),
    })),
  );

  function toggle(id: string): void {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    selected = next;
  }

  async function bulk(patch: { read?: boolean; done?: boolean; archived?: boolean }): Promise<void> {
    const ids = [...selected];
    if (ids.length === 0 || !account) return;
    await api.setNotificationState(account, ids, patch);
    selected = new Set();
    client.invalidateQueries({ queryKey: ["notifications"] });
  }

  async function one(id: string, patch: { read?: boolean; done?: boolean; archived?: boolean }): Promise<void> {
    if (!account) return;
    await api.setNotificationState(account, [id], patch);
    client.invalidateQueries({ queryKey: ["notifications"] });
  }
</script>

<div class="wrap">
  <div class="toolbar">
    <select bind:value={reason}>
      <option value="">All reasons</option>
      <option value="review_requested">Review requested</option>
      <option value="mention">Mention</option>
      <option value="ci_activity">CI activity</option>
    </select>
    <label class="check">
      <input type="checkbox" bind:checked={showArchived} /> Archived
    </label>
    <div class="spacer"></div>
    <button disabled={selected.size === 0} onclick={() => bulk({ read: true })}>Read</button>
    <button disabled={selected.size === 0} onclick={() => bulk({ done: true })}>Done</button>
    <button disabled={selected.size === 0} onclick={() => bulk({ archived: true })}>Archive</button>
  </div>

  {#if $query.isLoading}
    <div class="msg">Loading…</div>
  {:else if $query.isError}
    <div class="msg err">{($query.error as Error).message}</div>
  {:else if ($query.data?.items.length ?? 0) === 0}
    <div class="msg">Inbox zero.</div>
  {:else}
    <ul class="list">
      {#each $query.data?.items ?? [] as item (item.payload ? notificationOf(item).id : item.synced_at)}
        {@const n = notificationOf(item)}
        <li class:unread={n.unread && !item.state.read}>
          <input type="checkbox" checked={selected.has(n.id)} onchange={() => toggle(n.id)} />
          <div class="body">
            <span class="subject">{n.subject.title}</span>
            <span class="sub">{n.repository?.full_name ?? ""} · {n.reason}</span>
          </div>
          <div class="actions">
            {#if !item.state.read}
              <button onclick={() => one(n.id, { read: true })}>read</button>
            {/if}
            {#if !item.state.done}
              <button onclick={() => one(n.id, { done: true })}>done</button>
            {/if}
            <button onclick={() => one(n.id, { archived: true })}>archive</button>
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
  .check {
    color: var(--gh-fg-muted);
    font-size: 12px;
    display: flex;
    align-items: center;
    gap: 4px;
  }
  select,
  button {
    background: var(--gh-bg-elev);
    border: 1px solid var(--gh-border);
    color: var(--gh-fg);
    border-radius: var(--gh-radius);
    padding: 2px 10px;
    cursor: pointer;
  }
  button:disabled {
    opacity: 0.4;
    cursor: default;
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
    min-width: 0;
  }
  .subject {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sub {
    color: var(--gh-fg-muted);
    font-size: 12px;
  }
  .actions {
    display: flex;
    gap: 4px;
  }
  .actions button {
    font-size: 11px;
    padding: 1px 8px;
  }
  .msg {
    padding: var(--gh-space-4);
    color: var(--gh-fg-muted);
  }
  .err {
    color: var(--gh-danger);
  }
</style>
