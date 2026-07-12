<script lang="ts">
  import { createMutation, useQueryClient } from "@tanstack/svelte-query";
  import { api, type Subscription } from "../api/client";
  import { getAccount } from "../api/config";
  import { pullPath } from "../router/route";
  import { router } from "../router/router.svelte";
  import ManageSubscriptions from "./ManageSubscriptions.svelte";
  import RepoPicker from "./RepoPicker.svelte";

  type Tab = "url" | "repos" | "manage";

  const client = useQueryClient();
  const account = getAccount() ?? undefined;
  const popoverId = "subscribe-menu-popover";

  let tab = $state<Tab>("url");
  let prUrl = $state("");

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

<button class="trigger" popovertarget={popoverId} aria-label="Subscribe">Subscribe</button>

<div class="panel" id={popoverId} popover="auto">
  <div class="tabs" role="tablist">
    <button class:active={tab === "url"} onclick={() => (tab = "url")}>PR URL</button>
    <button class:active={tab === "repos"} onclick={() => (tab = "repos")}>Repos</button>
    <button class:active={tab === "manage"} onclick={() => (tab = "manage")}>Manage</button>
  </div>

  <div class="content">
    {#if tab === "url"}
      <form class="url-form" onsubmit={submitUrl}>
        <input
          type="text"
          placeholder="github.com PR URL or owner/repo#n"
          bind:value={prUrl}
          spellcheck="false"
        />
        <button type="submit" disabled={$subscribePr.isPending || !prUrl.trim()}>
          {$subscribePr.isPending ? "Subscribing…" : "Subscribe"}
        </button>
      </form>
      {#if $subscribePr.isError}
        <p class="error">{$subscribePr.error.message}</p>
      {/if}
      <p class="hint">Subscribes and opens the PR once synced.</p>
    {:else if tab === "repos"}
      {#if account}
        <RepoPicker />
      {:else}
        <p class="hint">No account selected — cannot list repos.</p>
      {/if}
    {:else}
      <ManageSubscriptions />
    {/if}
  </div>
</div>

<style>
  .trigger {
    background: var(--gh-bg-inset);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
    color: var(--gh-fg);
    cursor: pointer;
    padding: var(--gh-space-1) var(--gh-space-2);
    font-size: 12px;
    font-family: inherit;
  }
  .trigger:hover {
    border-color: var(--gh-accent);
  }
  .panel {
    position: fixed;
    top: 44px;
    right: 8px;
    left: auto;
    margin: 0;
    width: 340px;
    max-height: 70vh;
    background: var(--gh-bg-elev);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
    color: var(--gh-fg);
    padding: var(--gh-space-2);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.3);
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-2);
    overflow: hidden;
  }
  .tabs {
    display: flex;
    gap: var(--gh-space-1);
    border-bottom: 1px solid var(--gh-border);
    padding-bottom: var(--gh-space-1);
  }
  .tabs button {
    background: transparent;
    border: none;
    border-radius: var(--gh-radius-sm);
    color: var(--gh-fg-muted);
    cursor: pointer;
    padding: 2px 8px;
    font-size: 12px;
    font-family: inherit;
  }
  .tabs button.active {
    color: var(--gh-fg);
    background: var(--gh-bg-inset);
  }
  .content {
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-2);
    min-height: 0;
  }
  .url-form {
    display: flex;
    gap: var(--gh-space-1);
  }
  .url-form input {
    flex: 1;
    background: var(--gh-bg-inset);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
    color: var(--gh-fg);
    padding: var(--gh-space-1) var(--gh-space-2);
    font-size: 12px;
  }
  .url-form button {
    background: var(--gh-accent);
    border: 1px solid var(--gh-accent);
    border-radius: var(--gh-radius);
    color: var(--gh-accent-fg);
    cursor: pointer;
    padding: 2px 10px;
    font-size: 12px;
    white-space: nowrap;
  }
  .url-form button:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .hint {
    color: var(--gh-fg-muted);
    font-size: 11px;
    margin: 0;
  }
  .error {
    color: var(--gh-danger);
    font-size: 12px;
    margin: 0;
  }
</style>
