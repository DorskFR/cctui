<script lang="ts">
  import { Button } from "@dorsk/tsumikit";
  import type { GithubPull } from "../api/types";
  import { hostContext } from "../plugin/host";
  import { buildReviewPrompt } from "../review/agent-prompt";

  interface Props {
    owner: string;
    repo: string;
    pull: GithubPull;
  }
  let { owner, repo, pull }: Props = $props();

  const host = hostContext();
  const openSpawn = host?.openSpawn;

  function spawn(): void {
    openSpawn?.({ prompt: buildReviewPrompt({ owner, repo, pull }) });
  }
</script>

{#if openSpawn}
  <Button size="sm" variant="default" data-action="review-with-agent" onclick={spawn}>
    Review with agent
  </Button>
{/if}
