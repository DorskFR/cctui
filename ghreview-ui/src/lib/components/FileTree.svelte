<script lang="ts">
  import type { DiffModel } from "../diff/parse";

  interface Props {
    model: DiffModel;
    focusRow: number;
    onselect: (rowIndex: number) => void;
  }
  let { model, focusRow, onselect }: Props = $props();

  function activeFile(row: number): number {
    let idx = -1;
    for (let i = 0; i < model.files.length; i++) {
      if (model.files[i].fileRowIndex <= row) idx = i;
      else break;
    }
    return idx;
  }

  const active = $derived(activeFile(focusRow));
</script>

<ul class="tree">
  {#each model.files as file, i (file.filename)}
    <li>
      <button class:on={i === active} onclick={() => onselect(file.fileRowIndex)}>
        <span class="name" title={file.filename}>{file.filename}</span>
        <span class="counts">
          <span class="add">+{file.additions}</span>
          <span class="del">−{file.deletions}</span>
        </span>
      </button>
    </li>
  {/each}
</ul>

<style>
  .tree {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  button {
    width: 100%;
    display: flex;
    align-items: center;
    gap: var(--gh-space-2);
    background: transparent;
    border: none;
    color: var(--gh-fg-muted);
    padding: 3px var(--gh-space-3);
    cursor: pointer;
    text-align: left;
    font-size: 12px;
  }
  button.on {
    background: var(--gh-bg-elev);
    color: var(--gh-fg);
  }
  .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }
  .counts {
    font-family: var(--gh-mono);
    flex: none;
  }
  .add {
    color: var(--gh-success);
  }
  .del {
    color: var(--gh-danger);
  }
</style>
