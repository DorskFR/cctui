<script lang="ts">
  import { Button, Popover, Select, Textarea } from "@dorsk/tsumikit";
  import type { ReviewDraftComment, ReviewVerdict } from "../api/types";

  interface Skipped {
    path: string;
    line: number;
    reason: string;
  }

  interface Props {
    draftCount: number;
    drafts?: ReviewDraftComment[];
    publishing?: boolean;
    skipped?: Skipped[];
    error?: string | null;
    fullWidth?: boolean;
    onpublish: (verdict: ReviewVerdict, body: string) => void;
  }
  let {
    draftCount,
    drafts = [],
    publishing = false,
    skipped = [],
    error = null,
    fullWidth = false,
    onpublish,
  }: Props = $props();

  let verdict = $state<ReviewVerdict>("comment");
  let body = $state("");

  function publish(): void {
    onpublish(verdict, body.trim());
  }
</script>

<div class="bar" class:full-width={fullWidth}>
  <Popover
    label="Publish review"
    placement="bottom-end"
    size="sm"
    variant="default"
    block={fullWidth}
  >
    {#snippet trigger()}Review <span class="count">{draftCount}</span>{/snippet}
    <div class="panel">
      <label class="row">
        <span>Verdict</span>
        <Select bind:value={verdict} compact>
          <option value="comment">Comment</option>
          <option value="approve">Approve</option>
          <option value="request_changes">Request changes</option>
        </Select>
      </label>
      <Textarea bind:value={body} rows={3} mono placeholder="Review summary (optional)" />

      {#if drafts.length > 0}
        <details class="preview" open>
          <summary>Pending comments ({drafts.length})</summary>
          <ul>
            {#each drafts as d (d.id)}
              <li>
                <span class="loc">{d.path}:{d.line}</span>
                <span class="snippet">{d.body}</span>
              </li>
            {/each}
          </ul>
        </details>
      {/if}

      {#if error}
        <div class="err">{error}</div>
      {/if}

      {#if skipped.length > 0}
        <div class="skipped">
          <div class="skipped-head">{skipped.length} comment(s) skipped:</div>
          <ul>
            {#each skipped as s (`${s.path}:${s.line}`)}
              <li>{s.path}:{s.line} — {s.reason}</li>
            {/each}
          </ul>
        </div>
      {/if}

      <div class="actions">
        <Button
          size="sm"
          variant="primary"
          tone="accent"
          data-action="publish-review"
          disabled={publishing || (draftCount === 0 && verdict === "comment" && !body.trim())}
          onclick={publish}
        >
          {publishing ? "Publishing…" : "Publish review"}
        </Button>
      </div>
    </div>
  </Popover>
</div>

<style>
  .count {
    display: inline-block;
    min-width: 16px;
    text-align: center;
    background: color-mix(in srgb, currentColor 14%, transparent);
    border-radius: 999px;
    padding: 0 5px;
    margin-left: 4px;
    font-family: var(--gh-mono);
  }
  .panel {
    width: 320px;
    padding: var(--gh-space-2);
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-2);
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-size: var(--fs-xs);
    color: var(--gh-fg-muted);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
  }
  .err {
    font-size: var(--fs-xs);
    color: var(--gh-danger);
  }
  .skipped {
    font-size: var(--fs-xs);
    color: var(--gh-fg-muted);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius-sm);
    padding: var(--gh-space-1);
  }
  .skipped ul {
    margin: 4px 0 0;
    padding-left: 16px;
  }
  .preview {
    font-size: var(--fs-xs);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius-sm);
    padding: var(--gh-space-1);
  }
  .preview summary {
    cursor: pointer;
    color: var(--gh-fg-muted);
    user-select: none;
  }
  .preview ul {
    margin: var(--gh-space-1) 0 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: var(--gh-space-1);
    max-height: 12rem;
    overflow-y: auto;
  }
  .preview li {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding-top: var(--gh-space-1);
    border-top: 1px solid var(--gh-border);
  }
  .preview .loc {
    color: var(--gh-fg-muted);
    font-family: var(--gh-mono);
  }
  .preview .snippet {
    color: var(--gh-fg);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  @media (max-width: 700px) {
    .bar.full-width {
      width: 100%;
      box-sizing: border-box;
    }
  }
</style>
