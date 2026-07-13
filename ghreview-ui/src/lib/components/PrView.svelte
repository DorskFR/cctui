<script lang="ts">
  import { untrack } from "svelte";
  import { SegmentedControl } from "@dorsk/tsumikit";
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
    type ReviewDraftResult,
    type ReviewPublishResult,
    type ReviewThreadList,
    type ReviewVerdict,
    type ViewedStateResult,
  } from "../api/types";
  import { collapseViewedFiles } from "../diff/collapse";
  import { buildDiffModel } from "../diff/parse";
  import { buildNavIndex } from "../diff/navindex";
  import { buildAnchors, type LineAddress } from "../review/anchors";
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
  import ReviewSummaryBar from "./ReviewSummaryBar.svelte";
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

  const draftQuery = createQuery(
    toStore(() => ({
      queryKey: keys.reviewDraft(owner, repo, number),
      queryFn: () => api.reviewDraft(owner, repo, number, account as string),
      enabled: account !== null,
    })),
  );
  const threadsQuery = createQuery(
    toStore(() => ({
      queryKey: keys.reviewThreads(owner, repo, number),
      queryFn: () => api.reviewThreads(owner, repo, number, account as string),
      enabled: account !== null,
    })),
  );

  const drafts = $derived($draftQuery.data?.draft?.comments ?? []);
  const threads = $derived($threadsQuery.data?.items ?? []);
  const anchors = $derived(buildAnchors(displayModel, drafts, threads));

  let reviewPending = $state(false);
  let publishSkipped = $state<ReviewPublishResult["skipped"]>([]);
  let publishError = $state<string | null>(null);

  function setDraft(result: ReviewDraftResult): void {
    queryClient.setQueryData(keys.reviewDraft(owner, repo, number), result);
  }

  async function addComment(addr: LineAddress, body: string): Promise<void> {
    if (!account) return;
    reviewPending = true;
    try {
      const result = await api.addReviewComment(owner, repo, number, {
        account,
        path: addr.path,
        side: addr.side,
        line: addr.line,
        start_line: addr.start_line ?? null,
        start_side: addr.start_side ?? null,
        body,
        head_sha: pull?.head?.sha,
      });
      setDraft(result);
    } finally {
      reviewPending = false;
    }
  }

  async function editComment(id: string, body: string): Promise<void> {
    if (!account) return;
    reviewPending = true;
    try {
      setDraft(await api.editReviewComment(owner, repo, number, id, { account, body }));
    } finally {
      reviewPending = false;
    }
  }

  async function deleteComment(id: string): Promise<void> {
    if (!account) return;
    reviewPending = true;
    try {
      setDraft(await api.deleteReviewComment(owner, repo, number, id, account));
    } finally {
      reviewPending = false;
    }
  }

  async function publishReview(verdict: ReviewVerdict, body: string): Promise<void> {
    if (!account) return;
    reviewPending = true;
    publishError = null;
    publishSkipped = [];
    try {
      const result = await api.publishReview(owner, repo, number, { account, verdict, body });
      publishSkipped = result.skipped;
      queryClient.invalidateQueries({ queryKey: keys.reviewDraft(owner, repo, number) });
      queryClient.invalidateQueries({ queryKey: keys.reviewThreads(owner, repo, number) });
    } catch (e) {
      publishError = (e as Error).message;
    } finally {
      reviewPending = false;
    }
  }

  const review = $derived({
    anchors,
    addComment,
    editComment,
    deleteComment,
    pending: reviewPending,
  });

  function selectFile(rowIndex: number, path: string): void {
    if (viewed.has(path) && !expandedPaths.has(path)) {
      expandedPaths = new Set(expandedPaths).add(path);
    }
    focusRow = rowIndex;
  }

  let focusRow = $state(0);

  $effect(() => {
    if (!pull) return;
    const title = pull.title;
    const status = {
      pr: prStateOf(pull),
      ci: ciStateOf(pull),
      mergeable: pull.mergeable ?? null,
    };
    untrack(() => {
      const id = tabs.open(owner, repo, number, title);
      tabs.setStatus(id, status);
      tabs.activate(id);
    });
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
  let diffMode = $state<"unified" | "split">("unified");
  const effectiveRenderer = $derived<RendererKind>(diffMode === "split" ? "dom" : rendererKind);
  const DiffComponent = $derived(getRenderer(effectiveRenderer)?.component);

  const diffModeOptions = [
    { value: "unified", label: "Unified" },
    { value: "split", label: "Split" },
  ];

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
        <span class="chip mono">
          {#if pull.additions != null || pull.deletions != null}
            <span class="add">+{pull.additions ?? 0}</span>
            <span class="del">−{pull.deletions ?? 0}</span>
            ·
          {/if}
          {files.length} files
        </span>
        {#if files.length > 0}
          <span class="chip mono">viewed {viewedCount}/{files.length}</span>
        {/if}
        <SegmentedControl
          options={diffModeOptions}
          bind:value={diffMode}
          size="sm"
          label="Diff layout"
        />
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
        <ReviewSummaryBar
          draftCount={drafts.length}
          publishing={reviewPending}
          skipped={publishSkipped}
          error={publishError}
          onpublish={publishReview}
        />
      </div>
    </header>

    <PrTabs active={activeTab} counts={{ diff: files.length }} onselect={selectTab} />

    <div class="content">
      {#if activeTab === "description"}
        <PrDescription
          body={pull.body}
          {owner}
          {repo}
          {number}
          account={account ?? undefined}
          reactions={pull.reactions ?? null}
        />
      {:else if activeTab === "conversation"}
        <PrConversation {pull} {owner} {repo} account={account ?? undefined} />
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
              {#key effectiveRenderer}
                <DiffComponent
                  model={displayModel}
                  {nav}
                  {focusRow}
                  {review}
                  mode={diffMode}
                  {owner}
                  {repo}
                  account={account ?? undefined}
                  onFocusRow={(r) => (focusRow = r)}
                />
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
  .add {
    color: var(--gh-success);
  }
  .del {
    color: var(--gh-danger);
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
