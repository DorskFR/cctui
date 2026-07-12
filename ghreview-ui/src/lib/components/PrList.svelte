<script lang="ts">
  import { createQuery } from "@tanstack/svelte-query";
  import { api } from "../api/client";
  import { getAccount } from "../api/config";
  import { ciStateOf, prStateOf, pullOf, repoOf } from "../api/types";
  import {
    collectAuthors,
    collectLabels,
    collectRepos,
    emptyCriteria,
    filterEntries,
    type PrEntry,
  } from "../filter/prfilter";
  import { pullPath } from "../router/route";
  import { router } from "../router/router.svelte";
  import { tabs } from "../stores/tabs.svelte";
  import FilterSearchBar from "./FilterSearchBar.svelte";
  import StatusDot from "./StatusDot.svelte";

  let criteria = $state({ ...emptyCriteria });

  const account = getAccount() ?? "";

  const query = createQuery({
    queryKey: ["pulls", "home", account],
    queryFn: async (): Promise<PrEntry[]> => {
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

  const entries = $derived($query.data ?? []);
  const repos = $derived(collectRepos(entries));
  const authors = $derived(collectAuthors(entries));
  const labels = $derived(collectLabels(entries));
  const filtered = $derived(filterEntries(entries, criteria, account));

  function open(e: PrEntry): void {
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
  <FilterSearchBar bind:criteria {repos} {authors} {labels} />

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
