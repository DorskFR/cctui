<script lang="ts">
  import { createMutation, createQuery, useQueryClient } from "@tanstack/svelte-query";
  import { Badge, Button, Input, Text } from "@dorsk/tsumikit";
  import { api, type GithubRepo } from "../api/client";
  import { getAccount } from "../api/config";

  const client = useQueryClient();
  const account = getAccount() ?? "";
  let filter = $state("");

  const repos = createQuery({
    queryKey: ["github-repos", account],
    queryFn: () => api.githubRepos(account),
    enabled: !!account,
  });

  const subscribe = createMutation({
    mutationFn: (fullName: string) => api.subscribe(fullName, "repo", account),
    onSuccess: () => {
      client.invalidateQueries({ queryKey: ["subscriptions"] });
      client.invalidateQueries({ queryKey: ["pulls"] });
    },
  });

  const items = $derived(($repos.data?.items ?? []) as GithubRepo[]);
  const filtered = $derived(
    filter.trim()
      ? items.filter((r) => r.full_name.toLowerCase().includes(filter.trim().toLowerCase()))
      : items,
  );
  const pending = $derived($subscribe.isPending ? $subscribe.variables : null);
</script>

<div class="repo-picker">
  <Input type="text" placeholder="Filter repos…" bind:value={filter} spellcheck="false" />
  {#if $repos.isPending}
    <Text size="sm" tone="muted">Loading repos…</Text>
  {:else if $repos.isError}
    <Text size="sm" tone="danger">{$repos.error.message}</Text>
  {:else if filtered.length === 0}
    <Text size="sm" tone="muted">No repos.</Text>
  {:else}
    <ul>
      {#each filtered as repo (repo.full_name)}
        <li>
          <span class="name" title={repo.full_name}>{repo.full_name}</span>
          {#if repo.private}<Badge size="sm" tone="neutral">private</Badge>{/if}
          <Button
            size="sm"
            variant="default"
            disabled={$subscribe.isPending}
            onclick={() => $subscribe.mutate(repo.full_name)}
          >
            {pending === repo.full_name ? "…" : "Subscribe"}
          </Button>
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
