<script lang="ts" module>
  import type { ActivityEvent } from "../api/types";

  export function relativeTime(iso: string | null): string {
    if (!iso) return "";
    const then = new Date(iso).getTime();
    if (Number.isNaN(then)) return "";
    const secs = Math.round((Date.now() - then) / 1000);
    const abs = Math.abs(secs);
    const units: [number, Intl.RelativeTimeFormatUnit][] = [
      [60, "second"],
      [3600, "minute"],
      [86400, "hour"],
      [604800, "day"],
      [2629800, "week"],
      [31557600, "month"],
      [Number.POSITIVE_INFINITY, "year"],
    ];
    const fmt = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
    let divisor = 1;
    for (const [limit, unit] of units) {
      if (abs < limit) return fmt.format(-Math.round(secs / divisor), unit);
      divisor = limit;
    }
    return "";
  }

  export type ActivityKind =
    | "commit"
    | "approved"
    | "changes"
    | "review"
    | "comment"
    | "label"
    | "person"
    | "force-push"
    | "merged"
    | "closed"
    | "reopened"
    | "renamed";

  export function kindOf(ev: ActivityEvent): ActivityKind {
    switch (ev.event) {
      case "committed":
        return "commit";
      case "reviewed":
        if (ev.detail?.state === "APPROVED") return "approved";
        if (ev.detail?.state === "CHANGES_REQUESTED") return "changes";
        return "review";
      case "commented":
        return "comment";
      case "labeled":
      case "unlabeled":
        return "label";
      case "review_requested":
      case "review_request_removed":
      case "assigned":
      case "unassigned":
        return "person";
      case "head_ref_force_pushed":
        return "force-push";
      case "merged":
        return "merged";
      case "closed":
        return "closed";
      case "reopened":
        return "reopened";
      case "renamed":
        return "renamed";
      default:
        return "comment";
    }
  }

  export function phraseOf(ev: ActivityEvent): string {
    const d = ev.detail ?? {};
    switch (ev.event) {
      case "committed":
        return d.message ? `pushed ${d.message}` : "pushed a commit";
      case "reviewed":
        if (d.state === "APPROVED") return "approved these changes";
        if (d.state === "CHANGES_REQUESTED") return "requested changes";
        if (d.state === "DISMISSED") return "dismissed a review";
        return "reviewed";
      case "commented":
        return "commented";
      case "labeled":
        return d.label ? `added the ${d.label.name} label` : "added a label";
      case "unlabeled":
        return d.label ? `removed the ${d.label.name} label` : "removed a label";
      case "review_requested":
        return `requested a review from ${d.reviewer?.login ?? d.team ?? "a reviewer"}`;
      case "review_request_removed":
        return `removed the review request for ${d.reviewer?.login ?? d.team ?? "a reviewer"}`;
      case "assigned":
        return `assigned ${d.assignee?.login ?? "someone"}`;
      case "unassigned":
        return `unassigned ${d.assignee?.login ?? "someone"}`;
      case "head_ref_force_pushed":
        return "force-pushed the branch";
      case "merged":
        return "merged this pull request";
      case "closed":
        return "closed this pull request";
      case "reopened":
        return "reopened this pull request";
      case "renamed":
        return d.from && d.to ? `renamed this from “${d.from}” to “${d.to}”` : "renamed this";
      default:
        return ev.event;
    }
  }

  const ICONS: Record<ActivityKind, string> = {
    commit:
      "M11.93 8.5a4.002 4.002 0 0 1-7.86 0H.75a.75.75 0 0 1 0-1.5h3.32a4.002 4.002 0 0 1 7.86 0h3.32a.75.75 0 0 1 0 1.5Zm-1.43-.75a2.5 2.5 0 1 0-5 0 2.5 2.5 0 0 0 5 0Z",
    approved:
      "M13.78 4.22a.75.75 0 0 1 0 1.06l-7.25 7.25a.75.75 0 0 1-1.06 0L2.22 9.28a.751.751 0 0 1 .018-1.042.751.751 0 0 1 1.042-.018L6 10.94l6.72-6.72a.75.75 0 0 1 1.06 0Z",
    changes:
      "M3.72 3.72a.75.75 0 0 1 1.06 0L8 6.94l3.22-3.22a.749.749 0 0 1 1.275.326.749.749 0 0 1-.215.734L9.06 8l3.22 3.22a.749.749 0 0 1-.326 1.275.749.749 0 0 1-.734-.215L8 9.06l-3.22 3.22a.751.751 0 0 1-1.042-.018.751.751 0 0 1-.018-1.042L6.94 8 3.72 4.78a.75.75 0 0 1 0-1.06Z",
    review:
      "M1 2.75C1 1.784 1.784 1 2.75 1h10.5c.966 0 1.75.784 1.75 1.75v7.5A1.75 1.75 0 0 1 13.25 12H9.06l-2.573 2.573A1.458 1.458 0 0 1 4 13.543V12H2.75A1.75 1.75 0 0 1 1 10.25Zm1.75-.25a.25.25 0 0 0-.25.25v7.5c0 .138.112.25.25.25h2a.75.75 0 0 1 .75.75v2.19l2.72-2.72a.749.749 0 0 1 .53-.22h4.5a.25.25 0 0 0 .25-.25v-7.5a.25.25 0 0 0-.25-.25Z",
    comment:
      "M1 2.75C1 1.784 1.784 1 2.75 1h10.5c.966 0 1.75.784 1.75 1.75v7.5A1.75 1.75 0 0 1 13.25 12H9.06l-2.573 2.573A1.458 1.458 0 0 1 4 13.543V12H2.75A1.75 1.75 0 0 1 1 10.25Zm1.75-.25a.25.25 0 0 0-.25.25v7.5c0 .138.112.25.25.25h2a.75.75 0 0 1 .75.75v2.19l2.72-2.72a.749.749 0 0 1 .53-.22h4.5a.25.25 0 0 0 .25-.25v-7.5a.25.25 0 0 0-.25-.25Z",
    label:
      "M1 7.775V2.75C1 1.784 1.784 1 2.75 1h5.025c.464 0 .91.184 1.238.513l6.25 6.25a1.75 1.75 0 0 1 0 2.474l-5.026 5.026a1.75 1.75 0 0 1-2.474 0l-6.25-6.25A1.752 1.752 0 0 1 1 7.775Zm1.5 0c0 .066.026.13.073.177l6.25 6.25a.25.25 0 0 0 .354 0l5.025-5.025a.25.25 0 0 0 0-.354l-6.25-6.25a.25.25 0 0 0-.177-.073H2.75a.25.25 0 0 0-.25.25ZM6 5a1 1 0 1 1 0 2 1 1 0 0 1 0-2Z",
    person:
      "M10.561 8.073a6.005 6.005 0 0 1 3.432 5.142.75.75 0 1 1-1.498.07 4.5 4.5 0 0 0-8.99 0 .75.75 0 0 1-1.498-.07 6.004 6.004 0 0 1 3.431-5.142 3.999 3.999 0 1 1 5.622 0ZM10.5 5a2.5 2.5 0 1 0-5 0 2.5 2.5 0 0 0 5 0Z",
    "force-push":
      "M5.45 5.154A4.25 4.25 0 0 0 9.25 7.5h1.378a2.251 2.251 0 1 1 0 1.5H9.25A5.734 5.734 0 0 1 5 7.123v3.505a2.25 2.25 0 1 1-1.5 0V5.372a2.25 2.25 0 1 1 1.95-.218ZM4.25 13.5a.75.75 0 1 0 0-1.5.75.75 0 0 0 0 1.5Zm8.5-4.5a.75.75 0 1 0 0-1.5.75.75 0 0 0 0 1.5ZM5 3.25a.75.75 0 1 0 0 .005V3.25Z",
    merged:
      "M5.45 5.154A4.25 4.25 0 0 0 9.25 7.5h1.378a2.251 2.251 0 1 1 0 1.5H9.25A5.734 5.734 0 0 1 5 7.123v3.505a2.25 2.25 0 1 1-1.5 0V5.372a2.25 2.25 0 1 1 1.95-.218ZM4.25 13.5a.75.75 0 1 0 0-1.5.75.75 0 0 0 0 1.5Zm8.5-4.5a.75.75 0 1 0 0-1.5.75.75 0 0 0 0 1.5ZM5 3.25a.75.75 0 1 0 0 .005V3.25Z",
    closed:
      "M3.25 1A2.25 2.25 0 0 1 4 5.372v5.256a2.251 2.251 0 1 1-1.5 0V5.372A2.251 2.251 0 0 1 3.25 1Zm9.5 5.5a.75.75 0 0 1 .75.75v3.378a2.251 2.251 0 1 1-1.5 0V7.25a.75.75 0 0 1 .75-.75Zm-2.03-5.273a.75.75 0 0 1 1.06 0l.97.97.97-.97a.748.748 0 0 1 1.265.332.75.75 0 0 1-.205.729l-.97.97.97.97a.751.751 0 0 1-.018 1.042.751.751 0 0 1-1.042.018l-.97-.97-.97.97a.749.749 0 0 1-1.275-.326.749.749 0 0 1 .215-.734l.97-.97-.97-.97a.75.75 0 0 1 0-1.06ZM2.5 3.25a.75.75 0 1 0 1.5 0 .75.75 0 0 0-1.5 0ZM3.25 12a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Zm9.5 0a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Z",
    reopened:
      "M1.5 3.25a2.25 2.25 0 1 1 3 2.122v5.256a2.251 2.251 0 1 1-1.5 0V5.372A2.25 2.25 0 0 1 1.5 3.25Zm5.677-.177L9.573.677A.25.25 0 0 1 10 .854V2.5h1A2.5 2.5 0 0 1 13.5 5v5.628a2.251 2.251 0 1 1-1.5 0V5a1 1 0 0 0-1-1h-1v1.646a.25.25 0 0 1-.427.177L7.177 3.427a.25.25 0 0 1 0-.354ZM3.75 2.5a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Zm0 9.5a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Zm8.25.75a.75.75 0 1 0 1.5 0 .75.75 0 0 0-1.5 0Z",
    renamed:
      "M11.013 1.427a1.75 1.75 0 0 1 2.474 0l1.086 1.086a1.75 1.75 0 0 1 0 2.474l-8.61 8.61c-.21.21-.47.364-.756.445l-3.251.93a.75.75 0 0 1-.927-.928l.929-3.25c.081-.286.235-.547.445-.758l8.61-8.61Zm.176 4.823L9.75 4.81l-6.286 6.287a.253.253 0 0 0-.064.108l-.558 1.953 1.953-.558a.253.253 0 0 0 .108-.064Zm1.238-3.763a.25.25 0 0 0-.354 0L10.811 3.75l1.439 1.44 1.263-1.263a.25.25 0 0 0 0-.354Z",
  };

  export function iconOf(ev: ActivityEvent): string {
    return ICONS[kindOf(ev)];
  }
</script>

<script lang="ts">
  import { createQuery } from "@tanstack/svelte-query";
  import { api } from "../api/client";
  import { keys } from "../api/queries";
  import Avatar from "./Avatar.svelte";

  interface Props {
    owner: string;
    repo: string;
    number: number;
    account?: string;
  }
  let { owner, repo, number, account }: Props = $props();

  const query = createQuery(() => ({
    queryKey: keys.activity(owner, repo, number, account),
    queryFn: () => api.activity(owner, repo, number, account as string),
    enabled: account != null,
  }));

  const items = $derived(query.data?.items ?? []);
</script>

<div class="activity">
  {#if !account}
    <p class="muted">No account.</p>
  {:else if query.isLoading}
    <p class="muted">Loading activity…</p>
  {:else if query.isError}
    <p class="err">{(query.error as Error).message}</p>
  {:else if items.length === 0}
    <p class="muted">No activity recorded yet.</p>
  {:else}
    <ul>
      {#each items as ev, i (`${ev.event}-${ev.created_at}-${i}`)}
        {@const kind = kindOf(ev)}
        <li>
          <span class="icon {kind}" aria-hidden="true">
            <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor"><path d={iconOf(ev)} /></svg>
          </span>
          {#if ev.actor}
            <Avatar user={{ login: ev.actor.login, avatar_url: ev.actor.avatar_url ?? undefined }} size={18} />
            <span class="who">{ev.actor.login}</span>
          {:else if ev.detail?.author_name}
            <span class="who">{ev.detail.author_name}</span>
          {/if}
          <span class="phrase">{phraseOf(ev)}</span>
          {#if ev.detail?.sha}<code class="sha">{ev.detail.sha}</code>{/if}
          {#if ev.event === "reviewed" && ev.detail?.body}
            <span class="body">{ev.detail.body}</span>
          {/if}
          <span class="when" title={ev.created_at ?? undefined}>{relativeTime(ev.created_at)}</span>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .activity {
    padding: var(--gh-space-3) var(--gh-space-4);
    overflow: auto;
    height: 100%;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-2);
  }
  li {
    display: flex;
    align-items: center;
    gap: var(--gh-space-2);
    font-size: var(--fs-sm);
  }
  .icon {
    flex: none;
    display: inline-flex;
    color: var(--gh-fg-muted);
  }
  .icon.approved {
    color: var(--gh-success);
  }
  .icon.changes {
    color: var(--gh-danger);
  }
  .icon.merged {
    color: var(--gh-merged);
  }
  .icon.closed {
    color: var(--gh-danger);
  }
  .who {
    font-weight: 600;
  }
  .phrase {
    color: var(--gh-fg-muted);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sha {
    font-family: var(--gh-mono);
    background: var(--gh-bg-inset);
    padding: 0 6px;
    border-radius: var(--gh-radius-sm);
    color: var(--gh-accent);
  }
  .body {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--gh-fg-muted);
    font-style: italic;
  }
  .when {
    margin-left: auto;
    flex: none;
    color: var(--gh-fg-muted);
    font-variant-numeric: tabular-nums;
  }
  .muted {
    color: var(--gh-fg-muted);
    margin: 0;
  }
  .err {
    color: var(--gh-danger);
    margin: 0;
  }
</style>
