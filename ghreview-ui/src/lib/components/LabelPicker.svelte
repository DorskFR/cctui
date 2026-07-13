<script lang="ts">
  import { Popover } from "@dorsk/tsumikit";
  import { createQuery } from "@tanstack/svelte-query";
  import { toStore } from "svelte/store";
  import { api } from "../api/client";
  import { queryClient } from "../api/queries";
  import type { GithubLabel, Label, PullRequestEnvelope } from "../api/types";

  interface Props {
    owner: string;
    repo: string;
    number: number;
    account?: string;
    labels: GithubLabel[];
  }
  let { owner, repo, number, account, labels }: Props = $props();

  let open = $state(false);
  let filter = $state("");
  let pending = $state<string | null>(null);

  const applied = $derived(new Set(labels.map((l) => l.name)));

  const repoLabels = createQuery(
    toStore(() => ({
      queryKey: ["repo-labels", owner, repo, account ?? "*"],
      queryFn: () => api.repoLabels(owner, repo, account as string),
      enabled: open && account != null,
    })),
  );

  const options = $derived(($repoLabels.data?.items ?? []) as Label[]);
  const filtered = $derived(
    filter.trim()
      ? options.filter((l) => l.name.toLowerCase().includes(filter.trim().toLowerCase()))
      : options,
  );

  function textColor(hex: string): string {
    const h = hex.replace("#", "");
    if (h.length < 6) return "var(--gh-fg)";
    const r = Number.parseInt(h.slice(0, 2), 16);
    const g = Number.parseInt(h.slice(2, 4), 16);
    const b = Number.parseInt(h.slice(4, 6), 16);
    const luminance = (0.299 * r + 0.587 * g + 0.114 * b) / 255;
    return luminance > 0.6 ? "#000000" : "#ffffff";
  }

  function patchLabels(next: Label[]): void {
    const key = ["pull", owner, repo, number];
    const prev = queryClient.getQueryData<PullRequestEnvelope>(key);
    if (!prev) return;
    const payload = { ...((prev.payload as Record<string, unknown>) ?? {}) };
    payload.labels = next;
    queryClient.setQueryData(key, { ...prev, payload });
  }

  async function toggle(name: string): Promise<void> {
    if (!account || pending) return;
    pending = name;
    try {
      const result = applied.has(name)
        ? await api.removePullLabel(owner, repo, number, account, name)
        : await api.addPullLabel(owner, repo, number, account, name);
      patchLabels(result.labels);
    } finally {
      pending = null;
    }
  }
</script>

<div class="labels">
  {#each labels as label (label.name)}
    <span
      class="chip"
      style:background={label.color ? `#${label.color}` : "var(--gh-bg-elev)"}
      style:color={label.color ? textColor(label.color) : "var(--gh-fg)"}
      title={label.description ?? label.name}
    >
      {label.name}
    </span>
  {/each}

  {#if account}
    <Popover
      label="Edit labels"
      placement="bottom-start"
      onopen={() => (open = true)}
      onclose={() => (open = false)}
    >
      {#snippet trigger()}<span class="edit" aria-hidden="true">🏷 +</span>{/snippet}
      <div class="panel">
        <input
          class="filter"
          type="text"
          placeholder="Filter labels…"
          bind:value={filter}
          spellcheck="false"
        />
        {#if $repoLabels.isLoading}
          <p class="muted">Loading labels…</p>
        {:else if $repoLabels.isError}
          <p class="err">{($repoLabels.error as Error).message}</p>
        {:else if filtered.length === 0}
          <p class="muted">No labels.</p>
        {:else}
          <ul>
            {#each filtered as label (label.name)}
              <li>
                <button
                  type="button"
                  class="opt"
                  disabled={pending != null}
                  aria-pressed={applied.has(label.name)}
                  onclick={() => toggle(label.name)}
                >
                  <span class="check">{applied.has(label.name) ? "✓" : ""}</span>
                  <span class="dot" style:background={`#${label.color}`}></span>
                  <span class="name">{label.name}</span>
                  {#if pending === label.name}<span class="muted">…</span>{/if}
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    </Popover>
  {/if}
</div>

<style>
  .labels {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--gh-space-1);
  }
  .chip {
    font-size: 12px;
    line-height: 1;
    padding: 2px 8px;
    border-radius: 999px;
    border: 1px solid color-mix(in srgb, currentColor 20%, transparent);
  }
  .edit {
    font-size: 12px;
    color: var(--gh-fg-muted);
  }
  .panel {
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-2);
    width: 260px;
    max-width: 80vw;
  }
  .filter {
    background: var(--gh-bg-inset);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
    color: var(--gh-fg);
    padding: var(--gh-space-1) var(--gh-space-2);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 300px;
    overflow: auto;
  }
  .opt {
    width: 100%;
    display: flex;
    align-items: center;
    gap: var(--gh-space-2);
    background: transparent;
    border: none;
    color: var(--gh-fg);
    padding: var(--gh-space-1) var(--gh-space-2);
    border-radius: var(--gh-radius);
    cursor: pointer;
    text-align: left;
  }
  .opt:hover {
    background: var(--gh-bg-elev);
  }
  .opt:disabled {
    cursor: default;
    opacity: 0.7;
  }
  .check {
    width: 12px;
    color: var(--gh-accent);
  }
  .dot {
    width: 10px;
    height: 10px;
    border-radius: 999px;
    flex: none;
  }
  .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .muted {
    color: var(--gh-fg-muted);
    font-size: 12px;
    margin: 0;
  }
  .err {
    color: var(--gh-danger);
    font-size: 12px;
    margin: 0;
  }
</style>
