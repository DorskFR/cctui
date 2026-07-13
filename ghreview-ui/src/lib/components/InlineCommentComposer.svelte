<script lang="ts">
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
  <textarea
    bind:value={body}
    {placeholder}
    rows="3"
    autofocus
    onkeydown={onKeydown}
  ></textarea>
  <div class="actions">
    <button type="button" class="ghost" onclick={oncancel}>Cancel</button>
    <button type="button" class="primary" disabled={pending || !body.trim()} onclick={submit}>
      {submitLabel}
    </button>
  </div>
</div>

<style>
  .composer {
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-1);
  }
  textarea {
    width: 100%;
    box-sizing: border-box;
    font-family: var(--gh-mono);
    font-size: var(--fs-xs);
    background: var(--gh-bg);
    color: var(--gh-fg);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius-sm);
    padding: var(--gh-space-1);
    resize: vertical;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--gh-space-2);
  }
  button {
    font-size: var(--fs-xs);
    border-radius: var(--gh-radius-sm);
    padding: 2px 10px;
    cursor: pointer;
    border: 1px solid var(--gh-border);
  }
  .ghost {
    background: var(--gh-bg-inset);
    color: var(--gh-fg);
  }
  .primary {
    background: var(--gh-accent);
    color: white;
    border-color: var(--gh-accent);
  }
  .primary:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
