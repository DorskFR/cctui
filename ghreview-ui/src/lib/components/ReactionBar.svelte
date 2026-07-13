<script lang="ts">
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
    <button
      type="button"
      class="pill"
      class:mine={mine.has(content)}
      {disabled}
      aria-pressed={mine.has(content)}
      title={content}
      onclick={() => toggle(content)}
    >
      <span class="emoji">{EMOJI[content]}</span>
      <span class="count">{counts[content]}</span>
    </button>
  {/each}

  <div class="adder">
    <button
      type="button"
      class="add"
      {disabled}
      aria-label="Add reaction"
      aria-expanded={menuOpen}
      onclick={() => (menuOpen = !menuOpen)}
    >
      🙂<span class="plus">+</span>
    </button>
    {#if menuOpen}
      <div class="menu" role="menu">
        {#each ORDER as content (content)}
          <button
            type="button"
            class="opt"
            class:mine={mine.has(content)}
            role="menuitem"
            title={content}
            onclick={() => toggle(content)}
          >
            {EMOJI[content]}
          </button>
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
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
    line-height: 1;
    padding: 2px 8px;
    border-radius: 999px;
    border: 1px solid var(--gh-border);
    background: var(--gh-bg-elev);
    color: var(--gh-fg);
    cursor: pointer;
  }
  .pill.mine {
    border-color: var(--gh-accent);
    background: color-mix(in srgb, var(--gh-accent) 18%, transparent);
  }
  .pill:disabled,
  .add:disabled {
    cursor: default;
    opacity: 0.6;
  }
  .count {
    font-variant-numeric: tabular-nums;
  }
  .adder {
    position: relative;
    display: inline-flex;
  }
  .add {
    display: inline-flex;
    align-items: center;
    font-size: 12px;
    line-height: 1;
    padding: 2px 6px;
    border-radius: 999px;
    border: 1px solid var(--gh-border);
    background: var(--gh-bg-elev);
    color: var(--gh-fg-muted);
    cursor: pointer;
  }
  .plus {
    font-weight: 600;
    margin-left: 1px;
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
  .opt {
    font-size: 15px;
    line-height: 1;
    padding: 3px 5px;
    border-radius: var(--gh-radius);
    border: none;
    background: none;
    cursor: pointer;
  }
  .opt:hover,
  .opt.mine {
    background: color-mix(in srgb, var(--gh-accent) 22%, transparent);
  }
</style>
