<script lang="ts">
  import { createQuery, useQueryClient } from "@tanstack/svelte-query";
  import {
    Button,
    FilterSearchBar,
    Icon,
    IconButton,
    SegmentedControl,
    Text,
  } from "@dorsk/tsumikit";
  import { api } from "../api/client";
  import { getAccount } from "../api/config";
  import { keys } from "../api/queries";
  import { asGithubPull, ciStateOf, type GithubPull, prStateOf, pullOf, repoOf } from "../api/types";
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
  import Avatar from "./Avatar.svelte";
  import PrStateIcon, { stateColor } from "./PrStateIcon.svelte";
  import RepoBadge from "./RepoBadge.svelte";

  let query = $state("");
  let relation = $state("all");

  const account = getAccount() ?? "";
  const client = useQueryClient();

  const q = createQuery(() => ({
    queryKey: keys.pullsRoot(account),
    queryFn: async (): Promise<PrEntry[]> => {
      const repos = await api.allRepos(account || undefined);
      const results = await Promise.all(
        repos.map(async (env) => {
          const r = repoOf(env);
          const [owner, name] = r.full_name.split("/");
          const pulls = await api.allPulls(owner, name, env.account);
          return pulls.map((p) => ({ owner, repo: name, pull: pullOf(p) }));
        }),
      );
      return results.flat();
    },
  }));

  const snoozedQ = createQuery(() => ({
    queryKey: keys.pullsSnoozed(account),
    queryFn: async (): Promise<PrEntry[]> => {
      const res = await api.snoozedPulls(account || undefined);
      return res.items.map((s) => ({
        owner: s.owner,
        repo: s.repo,
        pull: asGithubPull(s.payload),
      }));
    },
  }));

  const isSnoozedView = $derived(relation === "snoozed");
  const active = $derived(isSnoozedView ? snoozedQ : q);
  const entries = $derived((active.data ?? []) as PrEntry[]);
  const schema = $derived(
    buildPrSchema(collectRepos(entries), collectAuthors(entries), collectLabels(entries)),
  );
  const filtered = $derived(
    filterPrs(entries, query, schema, (isSnoozedView ? "all" : relation) as PrRelation, account),
  );
  const groups = $derived(groupByRepo(filtered));

  const relationOptions = [
    { value: "all", label: "All" },
    { value: "review", label: "Review" },
    { value: "authored", label: "Authored" },
    { value: "snoozed", label: "Snoozed" },
  ];

  async function snooze(e: PrEntry): Promise<void> {
    if (!account) return;
    await api.snoozePull(e.owner, e.repo, e.pull.number, account);
    client.invalidateQueries({ queryKey: keys.pullsAll() });
  }

  async function unsnooze(e: PrEntry): Promise<void> {
    if (!account) return;
    await api.unsnoozePull(e.owner, e.repo, e.pull.number, account);
    client.invalidateQueries({ queryKey: keys.pullsAll() });
  }

  function isApproved(pull: GithubPull): boolean {
    const p = pull as unknown as Record<string, unknown>;
    return (p.review_decision ?? p.reviewDecision) === "APPROVED";
  }

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

  {#if active.isLoading}
    <div class="msg">Loading warm cache…</div>
  {:else if active.isError}
    <div class="msg err">{(active.error as Error).message}</div>
  {:else if filtered.length === 0}
    <div class="msg">
      {isSnoozedView ? "No snoozed pull requests." : "No pull requests match this filter."}
    </div>
  {:else}
    <div class="groups">
      {#each groups as group (group.repo)}
        <section class="group">
          <div class="group-head">
            <RepoBadge repo={group.repo} count={group.entries.length} />
          </div>
          <ul class="list">
            {#each group.entries as e (`${e.owner}/${e.repo}#${e.pull.number}`)}
              <li
                class:approved={isApproved(e.pull)}
                style:--marker={stateColor(prStateOf(e.pull))}
              >
                <Button
                  variant="ghost"
                  onclick={() => open(e)}
                  style="flex:1;min-width:0;justify-content:flex-start;gap:var(--gh-space-3);padding:var(--gh-space-2) var(--gh-space-3);text-align:left;border-radius:0"
                >
                  <PrStateIcon state={prStateOf(e.pull)} size={14} />
                  <span class="title">{e.pull.title}</span>
                  {#if e.pull.user}
                    <span class="author">
                      <Avatar user={e.pull.user} size={16} />
                      <Text as="span" size="xs" tone="muted">{e.pull.user.login}</Text>
                    </span>
                  {/if}
                  <Text as="span" size="xs" tone="muted" numeric>#{e.pull.number}</Text>
                  {#if e.pull.additions != null || e.pull.deletions != null}
                    <span class="counts">
                      <span class="add">+{e.pull.additions ?? 0}</span>
                      <span class="del">−{e.pull.deletions ?? 0}</span>
                    </span>
                  {/if}
                </Button>
                {#if isSnoozedView}
                  <IconButton
                    icon="moon"
                    label="Un-snooze this pull request"
                    variant="ghost"
                    size={16}
                    onclick={() => unsnooze(e)}
                  />
                {:else}
                  <IconButton
                    icon="moon"
                    label="Snooze this pull request"
                    variant="ghost"
                    size={16}
                    onclick={() => snooze(e)}
                  />
                {/if}
                {#if e.pull.html_url}
                  <a
                    class="ext"
                    href={e.pull.html_url}
                    target="_blank"
                    rel="noopener noreferrer"
                    aria-label="Open on GitHub"
                    title="Open on GitHub"
                  >
                    <Icon name="external" size={13} label="Open on GitHub" />
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
    border-left: 2px solid var(--marker, transparent);
  }
  li:last-child {
    border-bottom: none;
  }
  li.approved {
    background: color-mix(in srgb, var(--gh-success) 8%, transparent);
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
  .author {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    max-width: 40%;
    overflow: hidden;
  }
  .author > :global(span:last-child) {
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
