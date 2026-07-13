<script lang="ts">
  import { createQuery } from "@tanstack/svelte-query";
  import { FilterSearchBar, SegmentedControl, Text } from "@dorsk/tsumikit";
  import { api } from "../api/client";
  import { getAccount } from "../api/config";
  import { ciStateOf, prStateOf, pullOf, repoOf } from "../api/types";
  import {
    buildPrSchema,
    collectAuthors,
    collectLabels,
    collectRepos,
    filterPrs,
    groupByRepo,
    type PrEntry,
    type PrRelation,
  } from "../filter/prfilter";
  import { pullPath } from "../router/route";
  import { router } from "../router/router.svelte";
  import { tabs } from "../stores/tabs.svelte";
  import PrStateIcon from "./PrStateIcon.svelte";
  import RepoBadge from "./RepoBadge.svelte";

  let query = $state("");
  let relation = $state("all");

  const account = getAccount() ?? "";

  const q = createQuery({
    queryKey: ["pulls", "root", account],
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

  const entries = $derived($q.data ?? []);
  const schema = $derived(
    buildPrSchema(collectRepos(entries), collectAuthors(entries), collectLabels(entries)),
  );
  const filtered = $derived(filterPrs(entries, query, schema, relation as PrRelation, account));
  const groups = $derived(groupByRepo(filtered));

  const relationOptions = [
    { value: "all", label: "All" },
    { value: "review", label: "Review" },
    { value: "authored", label: "Authored" },
  ];

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
  <div class="filters">
    <FilterSearchBar
      {schema}
      bind:value={query}
      placeholder="Search title, author, repo, #number, label…"
      showChips
    />
    <SegmentedControl
      options={relationOptions}
      bind:value={relation}
      size="sm"
      label="Relation"
    />
  </div>

  {#if $q.isLoading}
    <div class="msg">Loading warm cache…</div>
  {:else if $q.isError}
    <div class="msg err">{($q.error as Error).message}</div>
  {:else if filtered.length === 0}
    <div class="msg">No pull requests match this filter.</div>
  {:else}
    <div class="groups">
      {#each groups as group (group.repo)}
        <section class="group">
          <div class="group-head">
            <RepoBadge repo={group.repo} count={group.entries.length} />
          </div>
          <ul class="list">
            {#each group.entries as e (`${e.owner}/${e.repo}#${e.pull.number}`)}
              <li>
                <button class="row" onclick={() => open(e)}>
                  <PrStateIcon state={prStateOf(e.pull)} size={14} />
                  <span class="title">{e.pull.title}</span>
                  <Text as="span" size="xs" tone="muted" numeric>#{e.pull.number}</Text>
                  {#if e.pull.additions != null || e.pull.deletions != null}
                    <span class="counts">
                      <span class="add">+{e.pull.additions ?? 0}</span>
                      <span class="del">−{e.pull.deletions ?? 0}</span>
                    </span>
                  {/if}
                </button>
                {#if e.pull.html_url}
                  <a
                    class="ext"
                    href={e.pull.html_url}
                    target="_blank"
                    rel="noopener noreferrer"
                    aria-label="Open on GitHub"
                    title="Open on GitHub"
                  >
                    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/><polyline points="15 3 21 3 21 9"/><line x1="10" y1="14" x2="21" y2="3"/></svg>
                  </a>
                {/if}
              </li>
            {/each}
          </ul>
        </section>
      {/each}
    </div>
  {/if}
</div>

<style>
  .wrap {
    padding: var(--gh-space-3);
  }
  .filters {
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-2);
    margin-bottom: var(--gh-space-3);
  }
  .groups {
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-3);
  }
  .group-head {
    margin-bottom: var(--gh-space-1);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
    overflow: hidden;
  }
  li {
    display: flex;
    align-items: stretch;
    border-bottom: 1px solid var(--gh-border-muted);
  }
  li:last-child {
    border-bottom: none;
  }
  .row {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: var(--gh-space-3);
    background: transparent;
    border: none;
    color: var(--gh-fg);
    padding: var(--gh-space-2) var(--gh-space-3);
    cursor: pointer;
    text-align: left;
  }
  .row:hover {
    background: var(--gh-bg-elev);
  }
  .ext {
    display: flex;
    align-items: center;
    padding: 0 var(--gh-space-3);
    color: var(--gh-fg-muted);
  }
  .ext:hover {
    color: var(--gh-accent);
    background: var(--gh-bg-elev);
  }
  .title {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .counts {
    font-family: var(--gh-mono);
    font-size: var(--fs-xs);
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
