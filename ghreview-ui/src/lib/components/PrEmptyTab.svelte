<script lang="ts">
  interface Props {
    title: string;
    detail: string;
  }
  let { title, detail }: Props = $props();
</script>

<div class="empty">
  <div class="title">{title}</div>
  <div class="detail">{detail}</div>
</div>

<style>
  .empty {
    padding: var(--gh-space-4);
    color: var(--gh-fg-muted);
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-2);
  }
  .title {
    color: var(--gh-fg);
    font-weight: 600;
  }
  .detail {
    font-size: 13px;
    max-width: 60ch;
  }
</style>
