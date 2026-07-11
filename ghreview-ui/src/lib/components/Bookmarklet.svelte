<script lang="ts">
  import { bookmarkletSource } from "../bookmarklet";

  const origin = window.location.origin;
  const href = bookmarkletSource(origin);
</script>

<div class="wrap">
  <h1>Open GitHub PRs here</h1>
  <p>
    Drag this button to your bookmarks bar. On any <code>github.com</code> pull request
    page, click it to jump to the mirrored review at this host.
  </p>
  <p class="drag">
    <a class="bm" {href}>Review on gh-review</a>
  </p>
  <p>Or paste a GitHub PR URL into the box in the top bar.</p>
  <p class="src">
    <label for="src">Snippet</label>
    <textarea id="src" readonly rows="3">{href}</textarea>
  </p>
</div>

<style>
  .wrap {
    padding: var(--gh-space-4);
    max-width: 640px;
  }
  h1 {
    font-size: 18px;
  }
  code {
    font-family: var(--gh-mono);
    background: var(--gh-bg-inset);
    padding: 0 4px;
    border-radius: var(--gh-radius-sm);
  }
  .drag {
    margin: var(--gh-space-4) 0;
  }
  .bm {
    display: inline-block;
    background: var(--gh-accent);
    color: white;
    padding: var(--gh-space-2) var(--gh-space-4);
    border-radius: var(--gh-radius);
    font-weight: 600;
    cursor: grab;
  }
  .bm:hover {
    text-decoration: none;
  }
  textarea {
    width: 100%;
    background: var(--gh-bg-inset);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
    color: var(--gh-fg);
    font-family: var(--gh-mono);
    font-size: 11px;
    padding: var(--gh-space-2);
  }
  label {
    display: block;
    color: var(--gh-fg-muted);
    font-size: 12px;
    margin-bottom: var(--gh-space-1);
  }
</style>
