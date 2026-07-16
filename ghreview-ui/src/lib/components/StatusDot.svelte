<script lang="ts">
  import { Dot } from "@dorsk/tsumikit";
  import type { CiState, PrState } from "../api/types";

  interface Props {
    pr: PrState;
    ci: CiState;
    loading?: boolean;
  }
  let { pr, ci, loading = false }: Props = $props();

  const color = $derived.by(() => {
    if (loading) return "var(--gh-fg-subtle)";
    if (pr === "merged") return "var(--gh-merged)";
    if (pr === "closed") return "var(--gh-danger)";
    if (pr === "draft") return "var(--gh-draft)";
    if (ci === "failure") return "var(--gh-danger)";
    if (ci === "pending") return "var(--gh-warning)";
    if (ci === "success") return "var(--gh-success)";
    return "var(--gh-fg-muted)";
  });

  const title = $derived(`${pr}${ci !== "none" ? ` · ci ${ci}` : ""}`);
</script>

<Dot {color} glow={loading} {title} />
