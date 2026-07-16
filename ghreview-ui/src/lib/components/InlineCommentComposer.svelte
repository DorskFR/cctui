<script lang="ts">
  import { Button, Textarea } from "@dorsk/tsumikit";
  import { untrack } from "svelte";

  interface Props {
    placeholder?: string;
    initial?: string;
    submitLabel?: string;
    onsubmit: (body: string) => void;
    oncancel: () => void;
    pending?: boolean;
  }
  let {
    placeholder = "Leave a comment…",
    initial = "",
    submitLabel = "Add comment",
    onsubmit,
    oncancel,
    pending = false,
  }: Props = $props();

  let body = $state(untrack(() => initial));

  function submit(): void {
    const trimmed = body.trim();
    if (!trimmed) return;
    onsubmit(trimmed);
    body = "";
  }

  function onKeydown(e: KeyboardEvent): void {
    if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
      e.preventDefault();
      submit();
    } else if (e.key === "Escape") {
      e.preventDefault();
      oncancel();
    }
  }
</script>

<div class="composer">
  <!-- svelte-ignore a11y_autofocus -->
  <Textarea
    bind:value={body}
    {placeholder}
    rows={3}
    mono
    autoresize
    size="sm"
    autofocus
    onkeydown={onKeydown}
  />
  <div class="actions">
    <Button variant="ghost" size="sm" onclick={oncancel}>Cancel</Button>
    <Button variant="primary" size="sm" disabled={pending || !body.trim()} onclick={submit}>
      {submitLabel}
    </Button>
  </div>
</div>

<style>
  .composer {
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-1);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--gh-space-2);
  }
</style>
