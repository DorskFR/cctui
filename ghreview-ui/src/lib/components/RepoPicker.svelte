<script lang="ts">
  import { createMutation, createQuery, useQueryClient } from "@tanstack/svelte-query";
  import { Badge, Input, Switch, Text } from "@dorsk/tsumikit";
  import { api, type GithubRepo, type Subscription } from "../api/client";
  import { getAccount } from "../api/config";
  import { keys } from "../api/queries";

  const client = useQueryClient();
  const account = getAccount() ?? "";
  let filter = $state("");

  const repos = createQuery(() => ({
    queryKey: keys.githubRepos(account),
    queryFn: () => api.githubRepos(account),
    enabled: !!account,
  }));

  const subs = createQuery(() => ({
    queryKey: keys.subscriptions(account),
    queryFn: () => api.listSubscriptions(account || undefined),
    enabled: !!account,
  }));

  const subById = $derived.by(() => {
    const map = new Map<string, string>();
    for (const s of (subs.data?.items ?? []) as Subscription[]) {
      if (s.kind === "repo" && s.target) map.set(s.target, s.id);
    }
    return map;
  });

  const subscribe = createMutation(() => ({
    mutationFn: (fullName: string) => api.subscribe(fullName, "repo", account),
    onSuccess: () => {
      client.invalidateQueries({ queryKey: keys.subscriptionsAll() });
      client.invalidateQueries({ queryKey: keys.pullsAll() });
    },
  }));

  const unsubscribe = createMutation(() => ({
    mutationFn: (id: string) => api.unsubscribe(id),
    onSuccess: () => {
      client.invalidateQueries({ queryKey: keys.subscriptionsAll() });
      client.invalidateQueries({ queryKey: keys.pullsAll() });
    },
  }));

  const items = $derived((repos.data?.items ?? []) as GithubRepo[]);
  const filtered = $derived(
    filter.trim()
      ? items.filter((r) => r.full_name.toLowerCase().includes(filter.trim().toLowerCase()))
      : items,
  );
  const busy = $derived(subscribe.isPending || unsubscribe.isPending);

  function toggle(fullName: string): void {
    const id = subById.get(fullName);
    if (id) unsubscribe.mutate(id);
    else subscribe.mutate(fullName);
  }
</script>

<div class="repo-picker">
  <Input type="text" placeholder="Filter repos…" bind:value={filter} spellcheck="false" />
  {#if repos.isPending}
    <Text size="sm" tone="muted">Loading repos…</Text>
  {:else if repos.isError}
    <Text size="sm" tone="danger">{repos.error.message}</Text>
  {:else if filtered.length === 0}
    <Text size="sm" tone="muted">No repos.</Text>
  {:else}
    <ul>
      {#each filtered as repo (repo.full_name)}
        {@const subscribed = subById.has(repo.full_name)}
        <li>
          <span class="name" title={repo.full_name}>{repo.full_name}</span>
          {#if repo.private}<Badge size="sm" tone="neutral">private</Badge>{/if}
          <Switch
            checked={subscribed}
            label={`${subscribed ? "Unsubscribe from" : "Subscribe to"} ${repo.full_name}`}
            disabled={busy}
            onclick={() => toggle(repo.full_name)}
          />
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .repo-picker {
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-2);
    min-height: 0;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    overflow-y: auto;
    max-height: 360px;
    display: flex;
    flex-direction: column;
  }
  li {
    display: flex;
    align-items: center;
    gap: var(--gh-space-2);
    padding: var(--gh-space-2);
    border-radius: var(--gh-radius-sm);
    border-bottom: 1px solid var(--gh-border-muted);
  }
  li:last-child {
    border-bottom: none;
  }
  li:hover {
    background: var(--gh-bg-inset);
  }
  .name {
    flex: 1;
    font-size: var(--fs-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
