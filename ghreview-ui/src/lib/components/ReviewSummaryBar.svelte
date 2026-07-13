<script lang="ts">
  import type { ReviewVerdict } from "../api/types";

  interface Skipped {
    path: string;
    line: number;
    reason: string;
  }

  interface Props {
    draftCount: number;
    publishing?: boolean;
    skipped?: Skipped[];
    error?: string | null;
    fullWidth?: boolean;
    onpublish: (verdict: ReviewVerdict, body: string) => void;
  }
  let {
    draftCount,
    publishing = false,
    skipped = [],
    error = null,
    fullWidth = false,
    onpublish,
  }: Props = $props();

  let verdict = $state<ReviewVerdict>("comment");
  let body = $state("");
  let open = $state(false);

  function publish(): void {
    onpublish(verdict, body.trim());
  }
</script>

<div class="bar" class:full-width={fullWidth}>
  <button type="button" class="toggle" onclick={() => (open = !open)}>
    Review <span class="count">{draftCount}</span>
  </button>

  {#if open}
    <div class="panel">
      <label class="row">
        <span>Verdict</span>
        <select bind:value={verdict}>
          <option value="comment">Comment</option>
          <option value="approve">Approve</option>
          <option value="request_changes">Request changes</option>
        </select>
      </label>
      <textarea bind:value={body} rows="3" placeholder="Review summary (optional)"></textarea>

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
        <button
          type="button"
          class="primary"
          disabled={publishing || (draftCount === 0 && verdict === "comment" && !body.trim())}
          onclick={publish}
        >
          {publishing ? "Publishing…" : "Publish review"}
        </button>
      </div>
    </div>
  {/if}
</div>

<style>
  .bar {
    position: relative;
  }
  .toggle {
    font-size: var(--fs-xs);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
    background: var(--gh-accent);
    color: white;
    padding: 2px 10px;
    cursor: pointer;
  }
  .count {
    display: inline-block;
    min-width: 16px;
    text-align: center;
    background: rgba(255, 255, 255, 0.25);
    border-radius: 999px;
    padding: 0 5px;
    margin-left: 4px;
    font-family: var(--gh-mono);
  }
  .panel {
    position: absolute;
    right: 0;
    top: calc(100% + 6px);
    z-index: var(--z-drawer, 100);
    width: 320px;
    background: var(--gh-bg-elev);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.4);
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
  select,
  textarea {
    font-size: var(--fs-xs);
    background: var(--gh-bg);
    color: var(--gh-fg);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius-sm);
    padding: var(--gh-space-1);
  }
  textarea {
    width: 100%;
    box-sizing: border-box;
    resize: vertical;
    font-family: var(--gh-mono);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
  }
  .primary {
    font-size: var(--fs-xs);
    background: var(--gh-accent);
    color: white;
    border: 1px solid var(--gh-accent);
    border-radius: var(--gh-radius-sm);
    padding: 3px 12px;
    cursor: pointer;
  }
  .primary:disabled {
    opacity: 0.5;
    cursor: default;
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

  @media (max-width: 700px) {
    .bar.full-width,
    .bar.full-width .toggle {
      width: 100%;
      box-sizing: border-box;
    }
    .bar.full-width .toggle {
      display: flex;
      min-height: 2.5rem;
      align-items: center;
      justify-content: center;
    }
  }
</style>
