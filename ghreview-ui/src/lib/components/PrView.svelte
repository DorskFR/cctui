<script lang="ts">
  import { untrack } from "svelte";
  import { createMutation, createQuery } from "@tanstack/svelte-query";
  import { toStore } from "svelte/store";
  import { api } from "../api/client";
  import { keys, queryClient } from "../api/queries";
  import { applyOptimisticViewed, changedSinceViewed, viewedSet } from "../api/viewed";
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
  import { resolveKey, type KeymapState } from "../keyboard/keymap";
  import { router } from "../router/router.svelte";
  import {
    type PrContentTab,
    defaultPrTab,
    deserializePrTab,
    prTabStorageKey,
  } from "../stores/pr-tabs-core";
  import { tabs } from "../stores/tabs.svelte";
  import DiffView from "./DiffView.svelte";
  import PrComments from "./PrComments.svelte";
  import PrCommits from "./PrCommits.svelte";
  import PrDescription from "./PrDescription.svelte";
  import PrTabs from "./PrTabs.svelte";
  import PrDiffHeader from "./organisms/PrDiffHeader.svelte";
  import PrDiffLayout from "./organisms/PrDiffLayout.svelte";

  interface Props {
    owner: string;
    repo: string;
    number: number;
  }
  let { owner, repo, number }: Props = $props();

  const query = createQuery(
    toStore(() => ({
      queryKey: keys.pull(owner, repo, number),
      queryFn: () => api.pull(owner, repo, number),
      initialData: () =>
        queryClient.getQueryData<PullRequestEnvelope>(keys.pull(owner, repo, number)),
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
  let expandInitKey = $state<string | null>(null);

  $effect(() => {
    const data = $viewedQuery.data;
    if (!data || files.length === 0) return;
    const key = `${owner}/${repo}/${number}`;
    if (expandInitKey === key) return;
    untrack(() => {
      expandedPaths = changedSinceViewed(data, files);
      expandInitKey = key;
    });
  });

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

  let diffMode = $state<"unified" | "split">("unified");

  function onMerged(): void {
    router.navigate(tabs.closeMerged(owner, repo, number));
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
    <PrDiffHeader
      {owner}
      {repo}
      {number}
      account={account ?? undefined}
      {pull}
      {files}
      {viewedCount}
      draftCount={drafts.length}
      {drafts}
      publishing={reviewPending}
      skipped={publishSkipped}
      error={publishError}
      bind:diffMode
      onpublish={publishReview}
      onmerged={onMerged}
    />

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
      {:else if activeTab === "commits"}
        <PrCommits {pull} {owner} {repo} />
      {:else if activeTab === "comments"}
        <PrComments
          {owner}
          {repo}
          {number}
          account={account ?? undefined}
          inline={threads}
          inlineLoading={$threadsQuery.isLoading}
          inlineError={$threadsQuery.error as Error | null}
        />
      {:else}
        <PrDiffLayout
          model={displayModel}
          {focusRow}
          {viewed}
          onselect={selectFile}
          onToggleViewed={toggleViewed}
        >
          {#if files.length === 0}
            <div class="msg">No file patches in the synced payload.</div>
          {:else}
            <DiffView
              model={displayModel}
              {nav}
              {focusRow}
              {review}
              mode={diffMode}
              {owner}
              {repo}
              account={account ?? undefined}
              {viewed}
              onToggleViewed={toggleViewed}
              onFocusRow={(r) => (focusRow = r)}
            />
          {/if}
        </PrDiffLayout>
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
  .content {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .msg {
    padding: var(--gh-space-4);
    color: var(--gh-fg-muted);
  }
  .err {
    color: var(--gh-danger);
  }
</style>
