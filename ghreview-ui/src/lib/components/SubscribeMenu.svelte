<script lang="ts">
  import { createMutation, useQueryClient } from "@tanstack/svelte-query";
  import { Button, Input, Popover, SegmentedControl, Text } from "@dorsk/tsumikit";
  import { api, type Subscription } from "../api/client";
  import { getAccount } from "../api/config";
  import { pullPath } from "../router/route";
  import { router } from "../router/router.svelte";
  import ManageSubscriptions from "./ManageSubscriptions.svelte";
  import RepoPicker from "./RepoPicker.svelte";

  const client = useQueryClient();
  const account = getAccount() ?? undefined;

  let tab = $state("url");
  let prUrl = $state("");

  const tabOptions = [
    { value: "url", label: "PR URL" },
    { value: "repos", label: "Repos" },
    { value: "manage", label: "Manage" },
  ];

  const subscribePr = createMutation({
    mutationFn: (target: string) => api.subscribe(target, "pull_request", account),
    onSuccess: (sub: Subscription) => {
      client.invalidateQueries({ queryKey: ["subscriptions"] });
      client.invalidateQueries({ queryKey: ["pulls"] });
      prUrl = "";
      const parsed = /^([^/]+)\/([^/#]+)#(\d+)$/.exec(sub.target ?? "");
      if (parsed) router.navigate(pullPath(parsed[1], parsed[2], Number(parsed[3])));
    },
  });

  function submitUrl(e: SubmitEvent): void {
    e.preventDefault();
    const target = prUrl.trim();
    if (target) $subscribePr.mutate(target);
  }
</script>

<Popover label="Subscribe" placement="bottom-start">
  {#snippet trigger()}Subscribe{/snippet}
  <div class="panel">
    <SegmentedControl options={tabOptions} bind:value={tab} size="sm" label="Subscribe section" />

    {#if tab === "url"}
      <form class="url-form" onsubmit={submitUrl}>
        <Input
          type="text"
          placeholder="github.com PR URL or owner/repo#n"
          bind:value={prUrl}
          spellcheck="false"
        />
        <Button type="submit" variant="primary" size="sm" disabled={$subscribePr.isPending || !prUrl.trim()}>
          {$subscribePr.isPending ? "Subscribing…" : "Subscribe"}
        </Button>
      </form>
      {#if $subscribePr.isError}
        <Text size="xs" tone="danger">{$subscribePr.error.message}</Text>
      {/if}
      <Text size="xs" tone="muted">Subscribes and opens the PR once synced.</Text>
    {:else if tab === "repos"}
      {#if account}
        <RepoPicker />
      {:else}
        <Text size="xs" tone="muted">No account selected — cannot list repos.</Text>
      {/if}
    {:else}
      <ManageSubscriptions />
    {/if}
  </div>
</Popover>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-2);
    width: 340px;
    max-width: 80vw;
    min-height: 0;
  }
  .url-form {
    display: flex;
    gap: var(--gh-space-1);
    align-items: center;
  }
  .url-form :global(input) {
    flex: 1;
  }
</style>
