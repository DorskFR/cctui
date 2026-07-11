<script lang="ts">
  import { createQuery } from "@tanstack/svelte-query";
  import { toStore } from "svelte/store";
  import { api } from "../api/client";
  import { queryClient } from "../api/queries";
  import {
    ciStateOf,
    type GithubFile,
    type GithubPull,
    prStateOf,
    type PullRequestEnvelope,
    pullOf,
  } from "../api/types";
  import { buildDiffModel } from "../diff/parse";
  import { buildNavIndex } from "../diff/navindex";
  import { getRenderer } from "../diff/renderer";
  import { resolveKey, type KeymapState } from "../keyboard/keymap";
  import { tabs } from "../stores/tabs.svelte";
  import FileTree from "./FileTree.svelte";

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
  const files = $derived<GithubFile[]>(pull?.files ?? []);
  const model = $derived(buildDiffModel(files));
  const nav = $derived(buildNavIndex(model));

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

  const renderer = getRenderer("dom");
  const DiffComponent = renderer?.component;
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
      </div>
    </header>

    <div class="split">
      <aside class="tree">
        <FileTree {model} {focusRow} onselect={(r) => (focusRow = r)} />
      </aside>
      <section class="diff">
        {#if files.length === 0}
          <div class="msg">No file patches in the synced payload.</div>
        {:else if DiffComponent}
          <DiffComponent {model} {nav} {focusRow} onFocusRow={(r) => (focusRow = r)} />
        {/if}
      </section>
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
