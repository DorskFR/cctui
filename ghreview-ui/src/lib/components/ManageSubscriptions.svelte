<script lang="ts">
  import { createMutation, createQuery, useQueryClient } from "@tanstack/svelte-query";
  import { Button, Text } from "@dorsk/tsumikit";
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
    <Text size="sm" tone="muted">Loading…</Text>
  {:else if $subs.isError}
    <Text size="sm" tone="danger">{$subs.error.message}</Text>
  {:else if items.length === 0}
    <Text size="sm" tone="muted">No active subscriptions.</Text>
  {:else}
    <ul>
      {#each items as sub (sub.id)}
        <li>
          <span class="body">
            <span class="target" title={sub.target ?? ""}>{label(sub)}</span>
            <Text size="xs" tone="muted">{sub.account}</Text>
          </span>
          <Button
            variant="ghost"
            hoverDanger
            disabled={$remove.isPending}
            onclick={() => $remove.mutate(sub.id)}
          >
            {pending === sub.id ? "…" : "Unsubscribe"}
          </Button>
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
    max-height: 360px;
    display: flex;
    flex-direction: column;
  }
  li {
    display: flex;
    align-items: center;
    gap: var(--gh-space-2);
    padding: var(--gh-space-2);
    border-radius: var(--gh-radius-sm);
    border-bottom: 1px solid var(--gh-border-muted);
  }
  li:last-child {
    border-bottom: none;
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
    font-size: var(--fs-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
