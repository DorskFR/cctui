<script lang="ts">
  import { api } from "../api/client";
  import type { ReactionContent, ReactionRollup } from "../api/types";
  import { renderMarkdown } from "../markdown";
  import ReactionBar from "./ReactionBar.svelte";

  interface Props {
    body: string | null | undefined;
    owner?: string;
    repo?: string;
    number?: number;
    account?: string;
    reactions?: ReactionRollup | null;
  }
  let { body, owner, repo, number, account, reactions = null }: Props = $props();
  const text = $derived((body ?? "").trim());

  const canReact = $derived(
    owner !== undefined && repo !== undefined && number !== undefined && account !== undefined,
  );
</script>

<div class="prdesc">
  {#if text}
    <div class="body markdown">{@html renderMarkdown(text)}</div>
  {:else}
    <div class="msg">No description provided.</div>
  {/if}

  {#if canReact}
    <ReactionBar
      {reactions}
      onToggle={(content: ReactionContent) =>
        api.togglePullReaction(
          owner as string,
          repo as string,
          number as number,
          account as string,
          content,
        )}
    />
  {/if}
</div>

<style>
  .prdesc {
    padding: var(--gh-space-4);
    overflow: auto;
    height: 100%;
  }
  .body {
    max-width: 72ch;
  }
  .msg {
    color: var(--gh-fg-muted);
  }
</style>
