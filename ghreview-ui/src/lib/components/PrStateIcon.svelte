<script lang="ts" module>
  import type { IconName, PrState, Tone } from "@dorsk/tsumikit";

  export type IconState = PrState | "issue-open" | "issue-closed";

  // tsumikit declares PR_TONE inside GitRef.svelte but does not re-export it from
  // the barrel, and a deep import would escape the plugin runtime's single
  // `@dorsk/tsumikit` shim. Mirror it until TSU exports it.
  const PR_TONE: Record<PrState, Tone> = {
    open: "ok",
    merged: "accent",
    closed: "danger",
    draft: "neutral",
  };

  const STATE_TONE: Record<IconState, Tone> = {
    ...PR_TONE,
    "issue-open": PR_TONE.open,
    "issue-closed": PR_TONE.merged,
  };

  const STATE_GLYPH: Record<IconState, IconName> = {
    open: "pull-request",
    merged: "git-merge",
    closed: "pull-request",
    draft: "pull-request",
    "issue-open": "disc",
    "issue-closed": "check-circle",
  };

  const TONE_VAR: Record<Tone, string> = {
    neutral: "var(--text-muted)",
    ok: "var(--ok)",
    success: "var(--ok)",
    warn: "var(--warn)",
    danger: "var(--danger)",
    info: "var(--info)",
    accent: "var(--accent)",
  };

  export function stateToneColor(state: IconState): string {
    return TONE_VAR[STATE_TONE[state]];
  }
</script>

<script lang="ts">
  import { Icon } from "@dorsk/tsumikit";

  interface Props {
    state: IconState;
    size?: number;
    muted?: boolean;
    inherit?: boolean;
  }
  let { state, size = 16, muted = false, inherit = false }: Props = $props();

  const tone = $derived(inherit || muted ? undefined : STATE_TONE[state]);
  const override = $derived.by(() => {
    if (inherit) return "";
    if (muted) return "color: var(--gh-fg-muted)";
    // Icon leaves `neutral` on currentColor; the draft state needs the tone.
    return tone === "neutral" ? `color: ${stateToneColor(state)}` : "";
  });
</script>

<span class="icon">
  <Icon
    name={STATE_GLYPH[state]}
    {size}
    {tone}
    style={override}
    label={state.replace("-", " ")}
  />
</span>

<style>
  .icon {
    display: inline-flex;
    flex: none;
  }
</style>
