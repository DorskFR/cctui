<script lang="ts">
  import { createMutation, useQueryClient } from "@tanstack/svelte-query";
  import { Button, Card, Cluster, Field, Heading, Input, Stack, Text } from "@dorsk/tsumikit";
  import { api, type Subscription } from "../api/client";
  import { getAccount } from "../api/config";
  import { keys } from "../api/queries";
  import { pullPath } from "../router/route";
  import { router } from "../router/router.svelte";
  import RepoPicker from "./RepoPicker.svelte";

  const client = useQueryClient();
  const account = getAccount() ?? undefined;

  let prUrl = $state("");

  const subscribePr = createMutation({
    mutationFn: (target: string) => api.subscribe(target, "pull_request", account),
    onSuccess: (sub: Subscription) => {
      client.invalidateQueries({ queryKey: keys.subscriptionsAll() });
      client.invalidateQueries({ queryKey: keys.pullsAll() });
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

<div class="wrap">
  <Stack gap="var(--gh-space-4)">
    <Heading level={1} size="md">Subscriptions</Heading>

    <Card padding="md" surface="raised">
      <Stack gap="var(--gh-space-2)" as="form" onsubmit={submitUrl}>
        <Field label="Subscribe to a pull request" hint="Subscribes and opens the PR once synced.">
          <Cluster gap="var(--gh-space-2)" wrap={false}>
            <Input
              type="text"
              placeholder="github.com PR URL or owner/repo#n"
              bind:value={prUrl}
              spellcheck="false"
              style="flex: 1;"
            />
            <Button
              type="submit"
              variant="primary"
              disabled={$subscribePr.isPending || !prUrl.trim()}
            >
              {$subscribePr.isPending ? "Subscribing…" : "Subscribe"}
            </Button>
          </Cluster>
        </Field>
        {#if $subscribePr.isError}
          <Text size="xs" tone="danger">{$subscribePr.error.message}</Text>
        {/if}
      </Stack>
    </Card>

    <Card padding="md" surface="raised">
      <Stack gap="var(--gh-space-2)">
        <Heading level={2} size="sm">Repositories</Heading>
        <Text size="xs" tone="muted">Toggle a repository to subscribe or unsubscribe.</Text>
        {#if account}
          <RepoPicker />
        {:else}
          <Text size="sm" tone="muted">No account selected — cannot list repos.</Text>
        {/if}
      </Stack>
    </Card>
  </Stack>
</div>

<style>
  .wrap {
    padding: var(--gh-space-4);
    max-width: 720px;
    margin: 0 auto;
  }
</style>
