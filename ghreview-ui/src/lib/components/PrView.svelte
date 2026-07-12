<script lang="ts">
  import { createMutation, createQuery } from "@tanstack/svelte-query";
  import { toStore } from "svelte/store";
  import { api } from "../api/client";
  import { keys, queryClient } from "../api/queries";
  import { applyOptimisticViewed, viewedSet } from "../api/viewed";
  import {
    ciStateOf,
    type GithubFile,
    type GithubPull,
    prStateOf,
    type PullRequestEnvelope,
    pullOf,
    type ViewedStateResult,
  } from "../api/types";
  import { collapseViewedFiles } from "../diff/collapse";
  import { buildDiffModel } from "../diff/parse";
  import { buildNavIndex } from "../diff/navindex";
  import {
    getPreferredRendererKind,
    getRenderer,
    type RendererKind,
    setPreferredRendererKind,
  } from "../diff/renderer";
  import { resolveKey, type KeymapState } from "../keyboard/keymap";
  import {
    type PrContentTab,
    defaultPrTab,
    deserializePrTab,
    prTabStorageKey,
  } from "../stores/pr-tabs-core";
  import { tabs } from "../stores/tabs.svelte";
  import FileTree from "./FileTree.svelte";
  import PrChecks from "./PrChecks.svelte";
  import PrCommits from "./PrCommits.svelte";
  import PrConversation from "./PrConversation.svelte";
  import PrDescription from "./PrDescription.svelte";
  import PrTabs from "./PrTabs.svelte";

  interface Props {
    owner: string;
    repo: string;
    number: number;
  }
  let { owner, repo, number }: Props = $props();

  const query = createQuery(
    toStore(() => ({
      queryKey: ["pull", owner, repo, number],
      queryFn: () => api.pull(owner, repo, number),
      initialData: () =>
        queryClient.getQueryData<PullRequestEnvelope>(["pull", owner, repo, number]),
    })),
  );

  const pull = $derived<GithubPull | null>($query.data ? pullOf($query.data) : null);
  const account = $derived<string | null>($query.data?.account ?? null);
  const files = $derived<GithubFile[]>(pull?.files ?? []);
  const model = $derived(buildDiffModel(files));

  const viewedQuery = createQuery(
    toStore(() => ({
      queryKey: keys.pullViewed(owner, repo, number),
      queryFn: () => api.pullViewed(owner, repo, number, account as string),
      enabled: account !== null,
    })),
  );
  const viewed = $derived(viewedSet($viewedQuery.data));

  let expandedPaths = $state(new Set<string>());
  const displayModel = $derived(collapseViewedFiles(model, { viewed, expanded: expandedPaths }));
  const nav = $derived(buildNavIndex(displayModel));
  const viewedCount = $derived(model.files.filter((f) => viewed.has(f.filename)).length);

  const viewedMutation = createMutation(
    toStore(() => ({
      mutationFn: (vars: { paths: string[]; viewed: boolean }) =>
        api.setPullViewed(owner, repo, number, account as string, vars.paths, vars.viewed),
      onMutate: async (vars: { paths: string[]; viewed: boolean }) => {
        const key = keys.pullViewed(owner, repo, number);
        await queryClient.cancelQueries({ queryKey: key });
        const previous = queryClient.getQueryData<ViewedStateResult>(key);
        queryClient.setQueryData(key, applyOptimisticViewed(previous, vars.paths, vars.viewed));
        return { previous };
      },
      onError: (_e: unknown, _vars: unknown, ctx: { previous?: ViewedStateResult } | undefined) => {
        queryClient.setQueryData(keys.pullViewed(owner, repo, number), ctx?.previous);
      },
      onSettled: () => {
        queryClient.invalidateQueries({ queryKey: keys.pullViewed(owner, repo, number) });
      },
    })),
  );

  function toggleViewed(paths: string[], next: boolean): void {
    if (!account) return;
    const still = new Set(expandedPaths);
    for (const p of paths) still.delete(p);
    expandedPaths = still;
    $viewedMutation.mutate({ paths, viewed: next });
  }

  function selectFile(rowIndex: number, path: string): void {
    if (viewed.has(path) && !expandedPaths.has(path)) {
      expandedPaths = new Set(expandedPaths).add(path);
    }
    focusRow = rowIndex;
  }

  let focusRow = $state(0);

  $effect(() => {
    if (!pull) return;
    const id = tabs.open(owner, repo, number, pull.title);
    tabs.setStatus(id, {
      pr: prStateOf(pull),
      ci: ciStateOf(pull),
      mergeable: pull.mergeable ?? null,
    });
    tabs.activate(id);
  });

  let keyState: KeymapState = { gPending: false };

  function onKeydown(e: KeyboardEvent): void {
    const res = resolveKey(e, keyState);
    keyState = res.state;
    const a = res.action;
    if (!a) return;
    if (a.type === "nextFile") {
      const t = nav.files.find((f) => f.rowIndex > focusRow);
      if (t) focusRow = t.rowIndex;
      e.preventDefault();
    } else if (a.type === "prevFile") {
      const cands = nav.files.filter((f) => f.rowIndex < focusRow);
      if (cands.length) focusRow = cands[cands.length - 1].rowIndex;
      e.preventDefault();
    } else if (a.type === "nextHunk") {
      const t = nav.hunks.find((h) => h.rowIndex > focusRow);
      if (t) focusRow = t.rowIndex;
      e.preventDefault();
    } else if (a.type === "prevHunk") {
      const cands = nav.hunks.filter((h) => h.rowIndex < focusRow);
      if (cands.length) focusRow = cands[cands.length - 1].rowIndex;
      e.preventDefault();
    } else if (a.type === "gotoDiff") {
      focusRow = nav.hunks[0]?.rowIndex ?? 0;
      e.preventDefault();
    }
  }

  let activeTab = $state<PrContentTab>(defaultPrTab());
  let tabLoaded = false;

  $effect(() => {
    const key = prTabStorageKey(owner, repo, number);
    if (!tabLoaded) {
      activeTab = deserializePrTab(localStorage.getItem(key));
      tabLoaded = true;
    }
  });

  function selectTab(tab: PrContentTab): void {
    activeTab = tab;
    localStorage.setItem(prTabStorageKey(owner, repo, number), tab);
  }

  let rendererKind = $state<RendererKind>(getPreferredRendererKind());
  const DiffComponent = $derived(getRenderer(rendererKind)?.component);

  function toggleRenderer(kind: RendererKind): void {
    rendererKind = kind;
    setPreferredRendererKind(kind);
  }
</script>

<svelte:window onkeydown={onKeydown} />

<div class="prview">
  {#if !pull}
    {#if $query.isLoading}
      <div class="msg">Loading pull request…</div>
    {:else if $query.isError}
      <div class="msg err">{($query.error as Error).message}</div>
    {:else}
      <div class="msg">Not synced yet.</div>
    {/if}
  {:else}
    <header class="head">
      <div class="titlerow">
        <h1>{pull.title} <span class="num">#{number}</span></h1>
        <span class="state state-{prStateOf(pull)}">{prStateOf(pull)}</span>
      </div>
      <div class="branches">
        <code>{pull.base?.ref ?? "?"}</code> ← <code>{pull.head?.ref ?? "?"}</code>
      </div>
      <div class="chips">
        <span class="chip">CI: {ciStateOf(pull)}</span>
        <span class="chip">
          {pull.mergeable === true ? "mergeable" : pull.mergeable === false ? "conflicts" : "mergeability unknown"}
        </span>
        {#if pull.draft}<span class="chip">draft</span>{/if}
        <span class="chip mono">+{pull.additions ?? 0} −{pull.deletions ?? 0} · {files.length} files</span>
        {#if files.length > 0}
          <span class="chip mono">viewed {viewedCount}/{files.length}</span>
        {/if}
        <span class="renderer-toggle" role="group" aria-label="Diff renderer">
          <button
            type="button"
            class:active={rendererKind === "dom"}
            onclick={() => toggleRenderer("dom")}
          >DOM</button>
          <button
            type="button"
            class:active={rendererKind === "canvas"}
            onclick={() => toggleRenderer("canvas")}
          >Canvas</button>
        </span>
      </div>
    </header>

    <PrTabs active={activeTab} counts={{ diff: files.length }} onselect={selectTab} />

    <div class="content">
      {#if activeTab === "description"}
        <PrDescription body={pull.body} />
      {:else if activeTab === "conversation"}
        <PrConversation {pull} />
      {:else if activeTab === "commits"}
        <PrCommits {pull} />
      {:else if activeTab === "checks"}
        <PrChecks {pull} />
      {:else}
        <div class="split">
          <aside class="tree">
            <FileTree
              model={displayModel}
              {focusRow}
              {viewed}
              onselect={selectFile}
              onToggleViewed={toggleViewed}
            />
          </aside>
          <section class="diff">
            {#if files.length === 0}
              <div class="msg">No file patches in the synced payload.</div>
            {:else if DiffComponent}
              {#key rendererKind}
                <DiffComponent model={displayModel} {nav} {focusRow} onFocusRow={(r) => (focusRow = r)} />
              {/key}
            {/if}
          </section>
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .prview {
    display: flex;
    flex-direction: column;
    height: 100%;
  }
  .head {
    padding: var(--gh-space-3);
    border-bottom: 1px solid var(--gh-border);
  }
  .titlerow {
    display: flex;
    align-items: center;
    gap: var(--gh-space-3);
  }
  h1 {
    font-size: 16px;
    margin: 0;
    font-weight: 600;
  }
  .num {
    color: var(--gh-fg-muted);
    font-weight: 400;
  }
  .state {
    text-transform: capitalize;
    border-radius: 999px;
    padding: 1px 10px;
    font-size: 12px;
    color: white;
  }
  .state-open {
    background: var(--gh-success);
  }
  .state-draft {
    background: var(--gh-draft);
  }
  .state-merged {
    background: var(--gh-merged);
  }
  .state-closed {
    background: var(--gh-danger);
  }
  .branches {
    color: var(--gh-fg-muted);
    margin-top: var(--gh-space-1);
  }
  code {
    font-family: var(--gh-mono);
    background: var(--gh-bg-inset);
    padding: 0 6px;
    border-radius: var(--gh-radius-sm);
  }
  .chips {
    display: flex;
    gap: var(--gh-space-2);
    margin-top: var(--gh-space-2);
    flex-wrap: wrap;
  }
  .chip {
    font-size: 12px;
    color: var(--gh-fg-muted);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
    padding: 1px 8px;
  }
  .mono {
    font-family: var(--gh-mono);
  }
  .renderer-toggle {
    display: inline-flex;
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
    overflow: hidden;
  }
  .renderer-toggle button {
    font-size: 12px;
    padding: 1px 8px;
    background: transparent;
    color: var(--gh-fg-muted);
    border: none;
    cursor: pointer;
  }
  .renderer-toggle button.active {
    background: var(--gh-accent);
    color: white;
  }
  .content {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .split {
    flex: 1;
    display: grid;
    grid-template-columns: 260px 1fr;
    min-height: 0;
  }
  .tree {
    border-right: 1px solid var(--gh-border);
    overflow: auto;
  }
  .diff {
    min-width: 0;
    overflow: hidden;
  }
  .msg {
    padding: var(--gh-space-4);
    color: var(--gh-fg-muted);
  }
  .err {
    color: var(--gh-danger);
  }
</style>
