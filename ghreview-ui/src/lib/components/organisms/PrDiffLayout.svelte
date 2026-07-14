<script lang="ts">
  import { ResizablePanel } from "@dorsk/tsumikit";
  import type { Snippet } from "svelte";
  import type { DiffModel } from "../../diff/parse";
  import FileTree from "../FileTree.svelte";

  interface Props {
    model: DiffModel;
    focusRow: number;
    viewed: Set<string>;
    onselect: (rowIndex: number, path: string) => void;
    onToggleViewed: (paths: string[], viewed: boolean) => void;
    children: Snippet;
  }

  let { model, focusRow, viewed, onselect, onToggleViewed, children }: Props = $props();

  const WIDTH_KEY = "ghreview:filesWidth";
</script>

<div class="workspace">
  <ResizablePanel
    side="left"
    label="Changed files"
    width={280}
    minWidth={220}
    maxWidth={640}
    resizeStep={24}
    widthKey={WIDTH_KEY}
  >
    {#snippet panel()}
      <FileTree {model} {focusRow} {viewed} {onselect} {onToggleViewed} />
    {/snippet}

    <section class="diff" aria-label="Pull request diff">
      {@render children()}
    </section>
  </ResizablePanel>
</div>

<style>
  .workspace {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: grid;
  }
  .diff {
    min-width: 0;
    height: 100%;
    overflow: hidden;
  }
</style>
