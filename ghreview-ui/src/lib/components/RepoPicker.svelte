<script lang="ts">
  import { createMutation, createQuery, useQueryClient } from "@tanstack/svelte-query";
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
  <input
    class="filter"
    type="text"
    placeholder="Filter repos…"
    bind:value={filter}
    spellcheck="false"
  />
  {#if $repos.isPending}
    <p class="muted">Loading repos…</p>
  {:else if $repos.isError}
    <p class="error">{$repos.error.message}</p>
  {:else if filtered.length === 0}
    <p class="muted">No repos.</p>
  {:else}
    <ul>
      {#each filtered as repo (repo.full_name)}
        <li>
          <span class="name" title={repo.full_name}>
            {repo.full_name}
            {#if repo.private}<span class="tag">private</span>{/if}
          </span>
          <button
            type="button"
            disabled={$subscribe.isPending}
            onclick={() => $subscribe.mutate(repo.full_name)}
          >
            {pending === repo.full_name ? "…" : "Subscribe"}
          </button>
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
  .filter {
    background: var(--gh-bg-inset);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
    color: var(--gh-fg);
    padding: var(--gh-space-1) var(--gh-space-2);
    font-size: 12px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    overflow-y: auto;
    max-height: 240px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  li {
    display: flex;
    align-items: center;
    gap: var(--gh-space-2);
    padding: var(--gh-space-1) var(--gh-space-1);
    border-radius: var(--gh-radius-sm);
  }
  li:hover {
    background: var(--gh-bg-inset);
  }
  .name {
    flex: 1;
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tag {
    font-size: 10px;
    color: var(--gh-fg-muted);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius-sm);
    padding: 0 4px;
    margin-left: 4px;
  }
  button {
    background: var(--gh-bg-inset);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
    color: var(--gh-fg);
    cursor: pointer;
    padding: 2px 8px;
    font-size: 11px;
  }
  button:hover:not(:disabled) {
    border-color: var(--gh-accent);
  }
  button:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .muted {
    color: var(--gh-fg-muted);
    font-size: 12px;
    margin: 0;
  }
  .error {
    color: var(--gh-danger);
    font-size: 12px;
    margin: 0;
  }
</style>
