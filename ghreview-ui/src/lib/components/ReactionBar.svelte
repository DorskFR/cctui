<script lang="ts">
  import { Badge, IconButton } from "@dorsk/tsumikit";
  import type { ReactionContent, ReactionRollup, ReactionSummary } from "../api/types";

  interface Props {
    reactions?: ReactionRollup | null;
    viewerReactions?: ReactionContent[];
    onToggle: (content: ReactionContent) => Promise<ReactionSummary>;
    disabled?: boolean;
  }
  let { reactions = null, viewerReactions = [], onToggle, disabled = false }: Props = $props();

  const ORDER: ReactionContent[] = [
    "+1",
    "-1",
    "laugh",
    "hooray",
    "confused",
    "heart",
    "rocket",
    "eyes",
  ];
  const EMOJI: Record<ReactionContent, string> = {
    "+1": "👍",
    "-1": "👎",
    laugh: "😄",
    hooray: "🎉",
    confused: "😕",
    heart: "❤️",
    rocket: "🚀",
    eyes: "👀",
  };

  let override = $state<{ counts: Record<ReactionContent, number>; mine: ReactionContent[] } | null>(
    null,
  );
  let pending = $state<ReactionContent | null>(null);
  let menuOpen = $state(false);

  function readCounts(r: ReactionRollup | null): Record<ReactionContent, number> {
    return {
      "+1": r?.["+1"] ?? 0,
      "-1": r?.["-1"] ?? 0,
      laugh: r?.laugh ?? 0,
      hooray: r?.hooray ?? 0,
      confused: r?.confused ?? 0,
      heart: r?.heart ?? 0,
      rocket: r?.rocket ?? 0,
      eyes: r?.eyes ?? 0,
    };
  }

  const counts = $derived(override?.counts ?? readCounts(reactions));
  const mine = $derived(new Set(override?.mine ?? viewerReactions));
  const visible = $derived(ORDER.filter((c) => counts[c] > 0));

  async function toggle(content: ReactionContent): Promise<void> {
    if (disabled || pending) return;
    menuOpen = false;
    pending = content;
    try {
      const summary = await onToggle(content);
      override = { counts: readCounts(summary), mine: summary.viewer_reactions ?? [] };
    } finally {
      pending = null;
    }
  }
</script>

<div class="reactions">
  {#each visible as content (content)}
    <Badge
      as="button"
      size="sm"
      active={mine.has(content)}
      class={mine.has(content) ? "pill mine" : "pill"}
      {disabled}
      aria-pressed={mine.has(content)}
      title={content}
      onclick={() => toggle(content)}
    >
      <span class="emoji">{EMOJI[content]}</span>
      <span class="count">{counts[content]}</span>
    </Badge>
  {/each}

  <div class="adder">
    <IconButton
      emoji="🙂"
      size={14}
      class="add"
      label="Add reaction"
      {disabled}
      aria-expanded={menuOpen}
      onclick={() => (menuOpen = !menuOpen)}
    />
    {#if menuOpen}
      <div class="menu" role="menu">
        {#each ORDER as content (content)}
          <IconButton
            emoji={EMOJI[content]}
            size={16}
            class={mine.has(content) ? "opt mine" : "opt"}
            role="menuitem"
            label={content}
            onclick={() => toggle(content)}
          />
        {/each}
      </div>
    {/if}
  </div>
</div>

<style>
  .reactions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--gh-space-1);
    margin-top: var(--gh-space-2);
  }
  .emoji {
    line-height: 1;
  }
  .count {
    font-variant-numeric: tabular-nums;
    margin-left: 4px;
  }
  .adder {
    position: relative;
    display: inline-flex;
  }
  .menu {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    z-index: 20;
    display: flex;
    gap: 2px;
    padding: 4px;
    border-radius: var(--gh-radius);
    border: 1px solid var(--gh-border);
    background: var(--gh-bg-elev);
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.4);
  }
</style>
