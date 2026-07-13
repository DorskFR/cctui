<script lang="ts">
  import { SegmentedControl } from "@dorsk/tsumikit";
  import type {
    GithubFile,
    GithubPull,
    ReviewPublishResult,
    ReviewVerdict,
  } from "../../api/types";
  import { ciStateOf, prStateOf } from "../../api/types";
  import Avatar from "../Avatar.svelte";
  import MergeButton from "../MergeButton.svelte";
  import Reviewers from "../Reviewers.svelte";
  import ReviewSummaryBar from "../ReviewSummaryBar.svelte";
  import PrHeaderIdentity from "../molecules/PrHeaderIdentity.svelte";

  interface Props {
    owner: string;
    repo: string;
    number: number;
    account?: string;
    pull: GithubPull;
    files: GithubFile[];
    viewedCount: number;
    draftCount: number;
    publishing?: boolean;
    skipped?: ReviewPublishResult["skipped"];
    error?: string | null;
    diffMode?: "unified" | "split";
    onpublish: (verdict: ReviewVerdict, body: string) => void;
  }

  let {
    owner,
    repo,
    number,
    account,
    pull,
    files,
    viewedCount,
    draftCount,
    publishing = false,
    skipped = [],
    error = null,
    diffMode = $bindable("unified"),
    onpublish,
  }: Props = $props();

  const state = $derived(prStateOf(pull));
  const mergeability = $derived(
    pull.mergeable === true
      ? "mergeable"
      : pull.mergeable === false
        ? "conflicts"
        : "mergeability unknown",
  );
  const diffModeOptions = [
    { value: "unified", label: "Unified" },
    { value: "split", label: "Split" },
  ];
</script>

<header class="pr-diff-header">
  <div class="title-row">
    <PrHeaderIdentity {owner} {repo} {number} {account} {pull} {state} />

    <div class="stats" aria-label="Pull request statistics">
      {#if pull.additions != null || pull.deletions != null}
        <span class="add">+{pull.additions ?? 0}</span>
        <span class="del">−{pull.deletions ?? 0}</span>
        <span class="separator">·</span>
      {/if}
      <span>{files.length} files</span>
      {#if files.length > 0}
        <span class="separator">·</span>
        <span>viewed <strong>{viewedCount}/{files.length}</strong></span>
      {/if}
      <span class="separator">·</span>
      <span>CI {ciStateOf(pull)}</span>
      <span class="separator">·</span>
      <span>{mergeability}</span>
    </div>
  </div>

  <div class="reviewers-row">
    <Reviewers {owner} {repo} {number} {account} />
  </div>

  <div class="divider"></div>

  <div class="lower-row">
    <div class="source">
      {#if pull.user}
        <span class="author">
          <Avatar user={pull.user} size={20} />
          <span>{pull.user.login}</span>
        </span>
      {/if}
      <span class="branches" aria-label="Base and head branches">
        <code>{pull.base?.ref ?? "?"}</code>
        <span class="branch-arrow" aria-hidden="true">←</span>
        <code>{pull.head?.ref ?? "?"}</code>
      </span>
    </div>

    <div class="actions" aria-label="Pull request actions">
      <div class="diff-mode">
        <SegmentedControl
          options={diffModeOptions}
          bind:value={diffMode}
          size="sm"
          label="Diff layout"
        />
      </div>
      <div class="review-action">
        <ReviewSummaryBar
          {draftCount}
          {publishing}
          {skipped}
          {error}
          fullWidth
          {onpublish}
        />
      </div>
      {#if state === "open" || state === "draft"}
        <div class="merge-action">
          <MergeButton {owner} {repo} {number} {account} {pull} fullWidth />
        </div>
      {/if}
    </div>
  </div>
</header>

<style>
  .pr-diff-header {
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-3);
    padding: var(--gh-space-3) var(--gh-space-4);
    border-bottom: 1px solid var(--gh-border);
    background: var(--gh-bg-elev);
  }
  .title-row,
  .lower-row,
  .source,
  .actions,
  .author,
  .branches {
    display: flex;
    align-items: center;
  }
  .title-row {
    gap: var(--gh-space-3);
    min-width: 0;
  }
  .stats {
    display: flex;
    flex: none;
    align-items: center;
    gap: 6px;
    color: var(--gh-fg-muted);
    font-family: var(--gh-mono);
    font-size: var(--fs-xs);
    white-space: nowrap;
  }
  .stats strong {
    color: var(--gh-fg);
    font-weight: 500;
  }
  .add {
    color: var(--gh-success);
  }
  .del {
    color: var(--gh-danger);
  }
  .separator,
  .branch-arrow {
    color: var(--gh-border-strong, var(--gh-border));
  }
  .reviewers-row {
    min-width: 0;
  }
  .divider {
    height: 1px;
    background: var(--gh-border);
  }
  .lower-row {
    justify-content: space-between;
    gap: var(--gh-space-4);
  }
  .source {
    min-width: 0;
    gap: var(--gh-space-4);
    color: var(--gh-fg-muted);
    font-size: var(--fs-xs);
  }
  .author,
  .branches {
    flex: none;
    gap: var(--gh-space-2);
  }
  code {
    max-width: 16rem;
    overflow: hidden;
    padding: 2px 7px;
    border-radius: var(--gh-radius-sm);
    background: var(--gh-bg-inset);
    color: var(--gh-fg-muted);
    font-family: var(--gh-mono);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .actions {
    flex: none;
    justify-content: flex-end;
    gap: var(--gh-space-2);
  }

  @media (max-width: 700px) {
    .pr-diff-header {
      gap: var(--gh-space-3);
      padding: var(--gh-space-3);
    }
    .title-row {
      display: contents;
    }
    .stats {
      order: 2;
      flex-wrap: wrap;
      white-space: normal;
    }
    .reviewers-row {
      order: 4;
      overflow-x: auto;
      scrollbar-width: thin;
    }
    .divider {
      order: 3;
    }
    .lower-row {
      display: contents;
    }
    .source {
      order: 1;
      flex-direction: column;
      align-items: flex-start;
      gap: var(--gh-space-2);
    }
    .branches {
      max-width: 100%;
      flex-wrap: wrap;
    }
    code {
      max-width: calc(50vw - var(--gh-space-4));
    }
    .actions {
      order: 5;
      display: grid;
      grid-template-columns: repeat(2, minmax(0, 1fr));
      width: 100%;
    }
    .diff-mode {
      grid-column: 1 / -1;
      display: flex;
      justify-content: center;
      padding: var(--gh-space-1);
      border: 1px solid var(--gh-border);
      border-radius: var(--gh-radius);
    }
    .review-action,
    .merge-action {
      min-width: 0;
    }
  }
</style>
