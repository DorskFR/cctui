<script lang="ts" module>
  export function repoHue(repo: string): number {
    let h = 0;
    for (let i = 0; i < repo.length; i++) h = (h * 31 + repo.charCodeAt(i)) >>> 0;
    return h % 360;
  }
</script>

<script lang="ts">
  import { Badge, Dot } from "@dorsk/tsumikit";

  interface Props {
    repo: string;
    count?: number;
  }
  const { repo, count }: Props = $props();
  const hue = $derived(repoHue(repo));
  const style = $derived(
    `background:hsl(${hue} 60% 50% / 0.14);color:hsl(${hue} 55% 72%);border-color:hsl(${hue} 55% 50% / 0.35)`,
  );
</script>

<Badge size="sm" {style} title={repo}>
  <Dot color="hsl({hue} 65% 58%)" />
  <span class="name">{repo}</span>
  {#if count !== undefined}<span class="count">{count}</span>{/if}
</Badge>

<style>
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .count {
    flex: none;
    font-variant-numeric: tabular-nums;
    color: var(--gh-fg-muted);
  }
</style>
