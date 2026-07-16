<script lang="ts">
  import { Badge, Cluster, SegmentedControl } from "@dorsk/tsumikit";
  import type {
    GithubFile,
    GithubPull,
    ReviewDraftComment,
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
    drafts?: ReviewDraftComment[];
    publishing?: boolean;
    skipped?: ReviewPublishResult["skipped"];
    error?: string | null;
    diffMode?: "unified" | "split";
    onpublish: (verdict: ReviewVerdict, body: string) => void;
    onmerged?: () => void;
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
    drafts = [],
    publishing = false,
    skipped = [],
    error = null,
    diffMode = $bindable("unified"),
    onpublish,
    onmerged,
  }: Props = $props();

  const state = $derived(prStateOf(pull));

  type BadgeTone = "neutral" | "ok" | "warn" | "danger" | "info";

  const ci = $derived(ciStateOf(pull));
  const ciTone = $derived<BadgeTone>(
    ci === "success"
      ? "ok"
      : ci === "failure"
        ? "danger"
        : ci === "pending"
          ? "warn"
          : "neutral",
  );

  const mergeability = $derived.by<{ text: string; tone: BadgeTone }>(() => {
    const s = pull.mergeable_state?.toLowerCase();
    if (pull.mergeable === false || s === "dirty") return { text: "conflicts", tone: "danger" };
    if (s === "blocked" || s === "behind" || s === "unstable" || s === "has_hooks") {
      return { text: s, tone: "warn" };
    }
    if (pull.mergeable === true || s === "clean") return { text: "mergeable", tone: "ok" };
    return { text: "mergeability unknown", tone: "neutral" };
  });

  const diffModeOptions = [
    { value: "unified", label: "Unified" },
    { value: "split", label: "Split" },
  ];
</script>

<header class="pr-diff-header">
  <div class="header-top">
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
        <Badge tone={ciTone} size="sm">CI {ci}</Badge>
        <Badge tone={mergeability.tone} size="sm">{mergeability.text}</Badge>
      </div>
    </div>

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
  </div>

  <div class="divider"></div>

  <div class="lower-row">
    <div class="reviewers-row">
      <Reviewers {owner} {repo} {number} {account} />
    </div>

    <Cluster
      class="actions"
      aria-label="Pull request actions"
      justify="flex-end"
      gap="var(--gh-space-2)"
    >
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
          {drafts}
          {publishing}
          {skipped}
          {error}
          {onpublish}
        />
      </div>
      {#if state === "open" || state === "draft"}
        <div class="merge-action">
          <MergeButton {owner} {repo} {number} {account} {pull} {onmerged} />
        </div>
      {/if}
    </Cluster>
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
  .author,
  .branches {
    display: flex;
    align-items: center;
  }
  .header-top {
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-1);
    min-width: 0;
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
  .diff-mode,
  .review-action,
  .merge-action {
    display: flex;
    align-items: center;
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
  @media (max-width: 700px) {
    .pr-diff-header {
      gap: var(--gh-space-3);
      padding: var(--gh-space-3);
    }
    .title-row {
      flex-wrap: wrap;
    }
    .stats {
      flex-wrap: wrap;
      white-space: normal;
    }
    .lower-row {
      flex-direction: column;
      align-items: stretch;
      gap: var(--gh-space-3);
    }
    .reviewers-row {
      overflow-x: auto;
      scrollbar-width: thin;
    }
    .source {
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
    .lower-row :global(.actions) {
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
