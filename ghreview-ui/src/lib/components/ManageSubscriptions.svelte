<script lang="ts">
  import { createMutation, createQuery, useQueryClient } from "@tanstack/svelte-query";
  import { api, type Subscription } from "../api/client";
  import { getAccount } from "../api/config";

  const client = useQueryClient();
  const account = getAccount() ?? undefined;

  const subs = createQuery({
    queryKey: ["subscriptions", account ?? null],
    queryFn: () => api.listSubscriptions(account),
  });

  const remove = createMutation({
    mutationFn: (id: string) => api.unsubscribe(id),
    onSuccess: () => {
      client.invalidateQueries({ queryKey: ["subscriptions"] });
      client.invalidateQueries({ queryKey: ["pulls"] });
    },
  });

  const items = $derived(($subs.data?.items ?? []) as Subscription[]);
  const pending = $derived($remove.isPending ? $remove.variables : null);

  function label(s: Subscription): string {
    if (s.kind === "repo") return `repo · ${s.target ?? "?"}`;
    if (s.kind === "pull_request") return `PR · ${s.target ?? "?"}`;
    return `${s.kind} · ${s.target ?? "—"}`;
  }
</script>

<div class="manage">
  {#if $subs.isPending}
    <p class="muted">Loading…</p>
  {:else if $subs.isError}
    <p class="error">{$subs.error.message}</p>
  {:else if items.length === 0}
    <p class="muted">No active subscriptions.</p>
  {:else}
    <ul>
      {#each items as sub (sub.id)}
        <li>
          <span class="body">
            <span class="target" title={sub.target ?? ""}>{label(sub)}</span>
            <span class="account">{sub.account}</span>
          </span>
          <button
            type="button"
            disabled={$remove.isPending}
            onclick={() => $remove.mutate(sub.id)}
          >
            {pending === sub.id ? "…" : "Unsubscribe"}
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .manage {
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    overflow-y: auto;
    max-height: 240px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  li {
    display: flex;
    align-items: center;
    gap: var(--gh-space-2);
    padding: var(--gh-space-1);
    border-radius: var(--gh-radius-sm);
  }
  li:hover {
    background: var(--gh-bg-inset);
  }
  .body {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .target {
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .account {
    font-size: 10px;
    color: var(--gh-fg-muted);
  }
  button {
    background: var(--gh-bg-inset);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
    color: var(--gh-fg);
    cursor: pointer;
    padding: 2px 8px;
    font-size: 11px;
  }
  button:hover:not(:disabled) {
    border-color: var(--gh-danger);
    color: var(--gh-danger);
  }
  button:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .muted {
    color: var(--gh-fg-muted);
    font-size: 12px;
    margin: 0;
  }
  .error {
    color: var(--gh-danger);
    font-size: 12px;
    margin: 0;
  }
</style>
