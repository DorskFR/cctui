<script lang="ts">
  import { createQuery, useQueryClient } from "@tanstack/svelte-query";
  import {
    Button,
    Checkbox,
    Cluster,
    OptionButton,
    SegmentedControl,
    Select,
  } from "@dorsk/tsumikit";
  import { toStore } from "svelte/store";
  import { api, type NotificationFilter } from "../api/client";
  import { getAccount, onConfigChange } from "../api/config";
  import { keys } from "../api/queries";
  import {
    type GithubNotification,
    type NotificationInboxItem,
    notificationOf,
    prStateOf,
    type PullRequestEnvelope,
    pullOf,
  } from "../api/types";
  import { parsePullApiUrl, pullPath } from "../router/route";
  import { router } from "../router/router.svelte";
  import { tabs } from "../stores/tabs.svelte";
  import PrStateIcon, { type IconState } from "./PrStateIcon.svelte";
  import RepoBadge from "./RepoBadge.svelte";

  const client = useQueryClient();

  let account = $state(getAccount() ?? "");
  $effect(() => onConfigChange(() => (account = getAccount() ?? "")));

  let reason = $state<string>("");
  let repoFilter = $state<string>("");
  let status = $state<string>("unread");
  let selected = $state<Set<string>>(new Set());
  let selectMode = $state<boolean>(false);

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
      queryKey: keys.notifications(filter),
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

  const visibleIds = $derived(visible.map((item) => notificationOf(item).id));
  const allSelected = $derived(
    visibleIds.length > 0 && visibleIds.every((id) => selected.has(id)),
  );
  const someSelected = $derived(visibleIds.some((id) => selected.has(id)));

  function toggleSelectAll(): void {
    const next = new Set(selected);
    if (allSelected) {
      for (const id of visibleIds) next.delete(id);
    } else {
      for (const id of visibleIds) next.add(id);
    }
    selected = next;
  }

  function onRowClick(n: GithubNotification): void {
    if (selectMode) toggle(n.id);
    else if (isPull(n)) openPull(n);
  }

  function isPull(n: GithubNotification): boolean {
    return n.subject.type === "PullRequest" && parsePullApiUrl(n.subject.url) !== null;
  }

  let pullStates = $state(new Map<string, IconState>());

  function readPullStates(list: NotificationInboxItem[]): Map<string, IconState> {
    const map = new Map<string, IconState>();
    for (const item of list) {
      const n = notificationOf(item);
      if (n.subject.type !== "PullRequest") continue;
      const ref = parsePullApiUrl(n.subject.url);
      if (!ref) continue;
      const env = client.getQueryData<PullRequestEnvelope>(
        keys.pull(ref.owner, ref.repo, ref.number),
      );
      if (env) map.set(n.id, prStateOf(pullOf(env)));
    }
    return map;
  }

  $effect(() => {
    const list = visible;
    const refresh = () => {
      pullStates = readPullStates(list);
    };
    refresh();
    return client.getQueryCache().subscribe(refresh);
  });

  function iconState(n: GithubNotification): { state: IconState; muted: boolean } | null {
    if (n.subject.type === "PullRequest") {
      const state = pullStates.get(n.id);
      return state ? { state, muted: false } : { state: "open", muted: true };
    }
    if (n.subject.type === "Issue") {
      return { state: "issue-open", muted: true };
    }
    return null;
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
    client.invalidateQueries({ queryKey: keys.notificationsAll() });
  }
</script>

{#snippet rowBody(n: GithubNotification)}
  <span class="subject">{n.subject.title}</span>
  <div class="sub">
    {#if n.repository?.full_name}
      <RepoBadge repo={n.repository.full_name} />
    {/if}
    <span class="reason">{n.reason}</span>
  </div>
{/snippet}

<div class="wrap">
  <div class="toolbar">
    <Cluster gap="var(--gh-space-2)" align="center">
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
      <OptionButton selected={selectMode} onclick={() => (selectMode = !selectMode)}>
        Select
      </OptionButton>
      <div class="spacer"></div>
      <Button size="sm" disabled={selected.size === 0} onclick={() => markDone([...selected])}>
        Mark done{selected.size ? ` (${selected.size})` : ""}
      </Button>
    </Cluster>
  </div>

  {#if repoCounts.length > 0}
    <div class="badges">
      <Cluster gap="var(--gh-space-2)" wrap>
        {#each repoCounts as [repo, count] (repo)}
          <OptionButton
            selected={repoFilter === repo}
            onclick={() => (repoFilter = repoFilter === repo ? "" : repo)}
          >
            <RepoBadge {repo} {count} />
          </OptionButton>
        {/each}
      </Cluster>
    </div>
  {/if}

  {#if $query.isLoading}
    <div class="msg">Loading…</div>
  {:else if $query.isError}
    <div class="msg err">{($query.error as Error).message}</div>
  {:else if visible.length === 0}
    <div class="msg">Inbox zero.</div>
  {:else}
    <div class="selectall">
      <Checkbox
        label="Select all"
        checked={allSelected}
        indeterminate={someSelected && !allSelected}
        onchange={toggleSelectAll}
      />
    </div>
    <ul class="list">
      {#each visible as item (notificationOf(item).id || item.synced_at)}
        {@const n = notificationOf(item)}
        {@const icon = iconState(n)}
        <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
        <li
          class:unread={n.unread && !item.state.read}
          class:selectmode={selectMode}
          class:selectedrow={selectMode && selected.has(n.id)}
          onclick={selectMode ? () => onRowClick(n) : undefined}
          role={selectMode ? "button" : undefined}
          tabindex={selectMode ? 0 : undefined}
          onkeydown={selectMode
            ? (e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  onRowClick(n);
                }
              }
            : undefined}
        >
          <span class="rowcheck" onclick={(e) => e.stopPropagation()} role="none">
            <Checkbox
              label={n.subject.title}
              checked={selected.has(n.id)}
              onchange={() => toggle(n.id)}
            />
          </span>
          {#if icon}
            <PrStateIcon state={icon.state} muted={icon.muted} size={16} />
          {/if}
          {#if isPull(n) && !selectMode}
            <button type="button" class="body open" onclick={() => openPull(n)}>
              {@render rowBody(n)}
            </button>
          {:else}
            <div class="body">{@render rowBody(n)}</div>
          {/if}
          <div class="actions" onclick={(e) => e.stopPropagation()} role="none">
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
    margin-bottom: var(--gh-space-3);
  }
  .spacer {
    flex: 1;
  }
  .badges {
    margin-bottom: var(--gh-space-3);
  }
  .selectall {
    display: flex;
    align-items: center;
    padding: var(--gh-space-1) var(--gh-space-3);
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
  li.selectmode {
    cursor: pointer;
  }
  li.selectmode:hover {
    background: var(--gh-bg-muted);
  }
  li.selectedrow {
    background: var(--gh-accent-subtle, var(--gh-bg-muted));
  }
  .rowcheck {
    display: inline-flex;
    align-items: center;
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
    font-size: var(--fs-xs);
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
