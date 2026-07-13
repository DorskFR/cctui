<script lang="ts">
  import { Popover } from "@dorsk/tsumikit";
  import { api } from "../api/client";
  import { keys, queryClient } from "../api/queries";
  import type { GithubPull, MergeMethod } from "../api/types";

  interface Props {
    owner: string;
    repo: string;
    number: number;
    account?: string;
    pull: GithubPull;
    fullWidth?: boolean;
    onmerged?: () => void;
  }
  let { owner, repo, number, account, pull, fullWidth = false, onmerged }: Props = $props();

  let method = $state<MergeMethod>("squash");
  let confirming = $state(false);
  let pending = $state(false);
  let error = $state<string | null>(null);

  const methods: { value: MergeMethod; label: string }[] = [
    { value: "squash", label: "Squash and merge" },
    { value: "merge", label: "Create a merge commit" },
    { value: "rebase", label: "Rebase and merge" },
  ];

  const mergeability = $derived(
    pull.mergeable === true
      ? { text: "Mergeable", cls: "ok" }
      : pull.mergeable === false
        ? { text: `Conflicts${pull.mergeable_state ? ` (${pull.mergeable_state})` : ""}`, cls: "bad" }
        : { text: "Mergeability unknown", cls: "muted" },
  );

  async function merge(): Promise<void> {
    if (!account || pending) return;
    pending = true;
    error = null;
    try {
      const result = await api.mergePull(owner, repo, number, {
        account,
        merge_method: method,
        expected_head_sha: pull.head?.sha,
      });
      if (!result.merged) throw new Error(result.message ?? "Pull request was not merged.");
      confirming = false;
      queryClient.invalidateQueries({ queryKey: keys.pull(owner, repo, number) });
      queryClient.invalidateQueries({ queryKey: ["pulls"] });
      onmerged?.();
    } catch (e) {
      error = (e as Error).message;
    } finally {
      pending = false;
    }
  }
</script>

{#if account}
  <div class="merge-button" class:full-width={fullWidth}>
    <Popover
      label="Merge pull request"
      placement="bottom-end"
      onclose={() => {
        confirming = false;
        error = null;
      }}
    >
      {#snippet trigger()}
        <span class="trigger" class:disabled={pull.draft}>Merge</span>
      {/snippet}
      <div class="panel">
        {#if pull.draft}
          <p class="muted">This pull request is a draft and cannot be merged.</p>
        {:else}
          <div class="state {mergeability.cls}">{mergeability.text}</div>
          <label class="row">
            <span>Method</span>
            <select bind:value={method} disabled={pending}>
              {#each methods as m (m.value)}
                <option value={m.value}>{m.label}</option>
              {/each}
            </select>
          </label>

          {#if error}
            <div class="err">{error}</div>
          {/if}

          {#if confirming}
            <div class="confirm">
              <span>Merge #{number} with {method}?</span>
              <div class="actions">
                <button type="button" class="ghost" disabled={pending} onclick={() => (confirming = false)}>
                  Cancel
                </button>
                <button type="button" class="primary" disabled={pending} onclick={merge}>
                  {pending ? "Merging…" : "Confirm merge"}
                </button>
              </div>
            </div>
          {:else}
            <div class="actions">
              <button type="button" class="primary" onclick={() => (confirming = true)}>
                Merge pull request
              </button>
            </div>
          {/if}
        {/if}
      </div>
    </Popover>
  </div>
{/if}

<style>
  .trigger {
    font-size: var(--fs-xs);
    background: var(--gh-success);
    color: white;
    border-radius: var(--gh-radius);
    padding: 2px 10px;
    cursor: pointer;
  }
  .trigger.disabled {
    background: var(--gh-bg-inset);
    color: var(--gh-fg-muted);
    cursor: default;
  }
  .panel {
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-2);
    width: 260px;
    max-width: 80vw;
  }
  .state {
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  .state.ok {
    color: var(--gh-success);
  }
  .state.bad {
    color: var(--gh-danger);
  }
  .state.muted {
    color: var(--gh-fg-muted);
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--gh-space-2);
    font-size: var(--fs-xs);
    color: var(--gh-fg-muted);
  }
  select {
    flex: 1;
    font-size: var(--fs-xs);
    background: var(--gh-bg);
    color: var(--gh-fg);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius-sm);
    padding: var(--gh-space-1);
  }
  .confirm {
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-1);
    font-size: var(--fs-xs);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--gh-space-2);
  }
  .primary {
    font-size: var(--fs-xs);
    background: var(--gh-success);
    color: white;
    border: 1px solid var(--gh-success);
    border-radius: var(--gh-radius-sm);
    padding: 3px 12px;
    cursor: pointer;
  }
  .primary:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .ghost {
    font-size: var(--fs-xs);
    background: transparent;
    color: var(--gh-fg);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius-sm);
    padding: 3px 12px;
    cursor: pointer;
  }
  .muted {
    color: var(--gh-fg-muted);
    font-size: var(--fs-xs);
    margin: 0;
  }
  .err {
    color: var(--gh-danger);
    font-size: var(--fs-xs);
  }

  @media (max-width: 700px) {
    .merge-button.full-width {
      display: grid;
      width: 100%;
    }
    .merge-button.full-width .trigger {
      display: flex;
      width: 100%;
      min-height: 2.5rem;
      box-sizing: border-box;
      align-items: center;
      justify-content: center;
    }
  }
</style>
