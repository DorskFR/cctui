<script lang="ts">
  import { Button, Checkbox } from "@dorsk/tsumikit";
  import type { DiffModel } from "../diff/parse";
  import {
    buildFileTree,
    collectFilePaths,
    isFullyViewed,
    type TreeNode,
    viewedProgress,
  } from "../diff/tree";

  interface Props {
    model: DiffModel;
    focusRow: number;
    viewed: Set<string>;
    onselect: (rowIndex: number, path: string) => void;
    onToggleViewed: (paths: string[], viewed: boolean) => void;
  }
  let { model, focusRow, viewed, onselect, onToggleViewed }: Props = $props();

  const tree = $derived(buildFileTree(model.files));

  function activeFile(row: number): number {
    let idx = -1;
    for (let i = 0; i < model.files.length; i++) {
      if (model.files[i].fileRowIndex <= row) idx = i;
      else break;
    }
    return idx;
  }
  const activeFilename = $derived(model.files[activeFile(focusRow)]?.filename ?? null);

  function toggle(node: TreeNode): void {
    const paths = collectFilePaths(node);
    onToggleViewed(paths, !isFullyViewed(node, viewed));
  }
</script>

{#snippet nodeView(node: TreeNode, depth: number)}
  {#if node.type === "dir"}
    {@const p = viewedProgress(node, viewed)}
    <li>
      <div class="dir" style:padding-left="{depth * 12 + 6}px">
        <Checkbox
          label="Mark folder {node.name} viewed"
          checked={p.total > 0 && p.viewed === p.total}
          indeterminate={p.viewed > 0 && p.viewed < p.total}
          onchange={() => toggle(node)}
        />
        <span class="dname">{node.name}</span>
        <span class="progress">{p.viewed}/{p.total}</span>
      </div>
      <ul>
        {#each node.children as child (child.path)}
          {@render nodeView(child, depth + 1)}
        {/each}
      </ul>
    </li>
  {:else}
    {@const on = node.path === activeFilename}
    {@const isViewed = viewed.has(node.path)}
    <li>
      <div class="file" style:padding-left="{depth * 12 + 6}px" class:on>
        <Checkbox
          label="Mark {node.path} viewed"
          checked={isViewed}
          onchange={() => onToggleViewed([node.path], !isViewed)}
        />
        <Button
          variant="ghost"
          block
          class="filebtn"
          style="flex: 1; min-width: 0; justify-content: flex-start; gap: var(--gh-space-2); padding: 3px 0; min-height: 0; height: auto; color: {on
            ? 'var(--gh-fg)'
            : 'var(--gh-fg-muted)'};"
          onclick={() => onselect(node.file.fileRowIndex, node.path)}
        >
          <span class="name" class:viewed={isViewed} title={node.path}>{node.name}</span>
          <span class="counts">
            <span class="add">+{node.file.additions}</span>
            <span class="del">−{node.file.deletions}</span>
          </span>
        </Button>
      </div>
    </li>
  {/if}
{/snippet}

<ul class="tree">
  {#each tree as node (node.path)}
    {@render nodeView(node, 0)}
  {/each}
</ul>

<style>
  .tree {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .dir :global(.label-text),
  .file :global(.label-text) {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
    border: 0;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .dir,
  .file {
    display: flex;
    align-items: center;
    gap: var(--gh-space-2);
    padding-right: var(--gh-space-3);
    font-size: var(--fs-xs);
  }
  .dir {
    color: var(--gh-fg);
    font-weight: 600;
    padding-top: 2px;
    padding-bottom: 2px;
  }
  .dname {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .progress {
    flex: none;
    color: var(--gh-fg-muted);
    font-family: var(--gh-mono);
    font-size: var(--fs-xs);
  }
  .file.on {
    background: var(--gh-bg-elev);
  }
  .name.viewed {
    text-decoration: line-through;
    opacity: 0.6;
  }
  .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
