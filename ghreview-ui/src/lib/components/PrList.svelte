<script lang="ts">
  import { createQuery } from "@tanstack/svelte-query";
  import { api } from "../api/client";
  import { getAccount } from "../api/config";
  import { ciStateOf, type GithubPull, prStateOf, pullOf, repoOf } from "../api/types";
  import { pullPath } from "../router/route";
  import { router } from "../router/router.svelte";
  import { tabs } from "../stores/tabs.svelte";
  import StatusDot from "./StatusDot.svelte";

  type Filter = "review" | "authored" | "all";
  let filter = $state<Filter>("review");
  let repoFilter = $state<string>("");

  const account = getAccount() ?? "";

  interface Entry {
    owner: string;
    repo: string;
    pull: GithubPull;
  }

  const query = createQuery({
    queryKey: ["pulls", "home", account],
    queryFn: async (): Promise<Entry[]> => {
      const repos = await api.repos(account || undefined);
      const results = await Promise.all(
        repos.items.map(async (env) => {
          const r = repoOf(env);
          const [owner, name] = r.full_name.split("/");
          const page = await api.pulls(owner, name, env.account);
          return page.items.map((p) => ({ owner, repo: name, pull: pullOf(p) }));
        }),
      );
      return results.flat();
    },
  });

  const repos = $derived.by(() => {
    const set = new Set<string>();
    for (const e of $query.data ?? []) set.add(`${e.owner}/${e.repo}`);
    return [...set].sort();
  });

  const filtered = $derived.by(() => {
    let entries = $query.data ?? [];
    if (repoFilter) entries = entries.filter((e) => `${e.owner}/${e.repo}` === repoFilter);
    if (filter === "authored") {
      entries = entries.filter((e) => e.pull.user?.login === account);
    } else if (filter === "review") {
      entries = entries.filter((e) =>
        (e.pull.requested_reviewers ?? []).some((u) => u.login === account),
      );
    }
    return entries;
  });

  function open(e: Entry): void {
    const p = e.pull;
    tabs.open(e.owner, e.repo, p.number, p.title);
    tabs.setStatus(`pr-${e.owner}-${e.repo}-${p.number}`, {
      pr: prStateOf(p),
      ci: ciStateOf(p),
      mergeable: p.mergeable ?? null,
    });
    router.navigate(pullPath(e.owner, e.repo, p.number));
  }
</script>

<div class="wrap">
  <div class="filters">
    <div class="chips">
      <button class:on={filter === "review"} onclick={() => (filter = "review")}>
        Review requested
      </button>
      <button class:on={filter === "authored"} onclick={() => (filter = "authored")}>
        Authored
      </button>
      <button class:on={filter === "all"} onclick={() => (filter = "all")}>All</button>
    </div>
    <select bind:value={repoFilter}>
      <option value="">All repos</option>
      {#each repos as r (r)}
        <option value={r}>{r}</option>
      {/each}
    </select>
  </div>

  {#if $query.isLoading}
    <div class="msg">Loading warm cache…</div>
  {:else if $query.isError}
    <div class="msg err">{($query.error as Error).message}</div>
  {:else if filtered.length === 0}
    <div class="msg">No pull requests match this filter.</div>
  {:else}
    <ul class="list">
      {#each filtered as e (`${e.owner}/${e.repo}#${e.pull.number}`)}
        <li>
          <button class="row" onclick={() => open(e)}>
            <StatusDot pr={prStateOf(e.pull)} ci={ciStateOf(e.pull)} />
            <span class="title">{e.pull.title}</span>
            <span class="meta">{e.owner}/{e.repo} #{e.pull.number}</span>
            <span class="counts">
              <span class="add">+{e.pull.additions ?? 0}</span>
              <span class="del">−{e.pull.deletions ?? 0}</span>
            </span>
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .wrap {
    padding: var(--gh-space-3);
  }
  .filters {
    display: flex;
    justify-content: space-between;
    gap: var(--gh-space-3);
    margin-bottom: var(--gh-space-3);
  }
  .chips {
    display: flex;
    gap: var(--gh-space-1);
  }
  .chips button {
    background: var(--gh-bg-elev);
    border: 1px solid var(--gh-border);
    color: var(--gh-fg-muted);
    border-radius: 999px;
    padding: 2px 12px;
    cursor: pointer;
    font-size: 12px;
  }
  .chips button.on {
    background: var(--gh-accent);
    color: white;
    border-color: var(--gh-accent);
  }
  select {
    background: var(--gh-bg-elev);
    border: 1px solid var(--gh-border);
    color: var(--gh-fg);
    border-radius: var(--gh-radius);
    padding: 2px 8px;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
    overflow: hidden;
  }
  .row {
    width: 100%;
    display: flex;
    align-items: center;
    gap: var(--gh-space-3);
    background: transparent;
    border: none;
    border-bottom: 1px solid var(--gh-border-muted);
    color: var(--gh-fg);
    padding: var(--gh-space-2) var(--gh-space-3);
    cursor: pointer;
    text-align: left;
  }
  .row:hover {
    background: var(--gh-bg-elev);
  }
  .title {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    color: var(--gh-fg-muted);
    font-size: 12px;
  }
  .counts {
    font-family: var(--gh-mono);
    font-size: 12px;
  }
  .add {
    color: var(--gh-success);
  }
  .del {
    color: var(--gh-danger);
  }
  .msg {
    padding: var(--gh-space-4);
    color: var(--gh-fg-muted);
  }
  .err {
    color: var(--gh-danger);
  }
</style>
