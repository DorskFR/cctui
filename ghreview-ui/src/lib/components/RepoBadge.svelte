<script lang="ts" module>
  export function repoHue(repo: string): number {
    let h = 0;
    for (let i = 0; i < repo.length; i++) h = (h * 31 + repo.charCodeAt(i)) >>> 0;
    return h % 360;
  }
</script>

<script lang="ts">
  interface Props {
    repo: string;
    count?: number;
  }
  const { repo, count }: Props = $props();
  const hue = $derived(repoHue(repo));
</script>

<span
  class="badge"
  style="--h: {hue}"
  title={repo}
>
  <span class="dot"></span>
  <span class="name">{repo}</span>
  {#if count !== undefined}<span class="count">{count}</span>{/if}
</span>

<style>
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    max-width: 100%;
    padding: 1px 8px;
    font-size: var(--fs-xs);
    line-height: 18px;
    border-radius: var(--gh-radius-sm);
    background: hsl(var(--h) 60% 50% / 0.14);
    color: hsl(var(--h) 55% 72%);
    border: 1px solid hsl(var(--h) 55% 50% / 0.35);
  }
  .dot {
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: hsl(var(--h) 65% 58%);
  }
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
