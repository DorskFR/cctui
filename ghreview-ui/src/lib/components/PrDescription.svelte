<script lang="ts">
  interface Props {
    body: string | null | undefined;
  }
  let { body }: Props = $props();
  const text = $derived((body ?? "").trim());
</script>

<div class="prdesc">
  {#if text}
    <div class="body">{text}</div>
  {:else}
    <div class="msg">No description provided.</div>
  {/if}
</div>

<style>
  .prdesc {
    padding: var(--gh-space-4);
    overflow: auto;
    height: 100%;
  }
  .body {
    white-space: pre-wrap;
    word-break: break-word;
    font-size: 14px;
    line-height: 1.6;
    max-width: 72ch;
  }
  .msg {
    color: var(--gh-fg-muted);
  }
</style>
