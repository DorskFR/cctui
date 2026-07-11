<script lang="ts">
  import { rewriteGithubUrl } from "../bookmarklet";
  import { router } from "../router/router.svelte";

  let prUrl = $state("");

  function openUrl(e: SubmitEvent): void {
    e.preventDefault();
    const target = rewriteGithubUrl(prUrl.trim(), window.location.origin);
    if (target) {
      router.navigate(new URL(target).pathname);
      prUrl = "";
    }
  }

  function toggleTheme(): void {
    const root = document.documentElement;
    const next = root.getAttribute("data-theme") === "light" ? "dark" : "light";
    root.setAttribute("data-theme", next);
  }
</script>

<header class="topbar">
  <button class="brand" onclick={() => router.navigate("/")}>gh-review</button>
  <form class="url" onsubmit={openUrl}>
    <input
      type="text"
      placeholder="Paste a github.com PR URL…"
      bind:value={prUrl}
      spellcheck="false"
    />
  </form>
  <div class="spacer"></div>
  <a href="/bookmarklet" onclick={(e) => { e.preventDefault(); router.navigate("/bookmarklet"); }}>
    Bookmarklet
  </a>
  <button class="ghost" onclick={toggleTheme} title="Toggle theme">◐</button>
</header>

<style>
  .topbar {
    display: flex;
    align-items: center;
    gap: var(--gh-space-3);
    padding: var(--gh-space-2) var(--gh-space-3);
    background: var(--gh-bg-elev);
    border-bottom: 1px solid var(--gh-border);
    z-index: var(--gh-z-header);
  }
  .brand {
    background: transparent;
    border: none;
    color: var(--gh-fg);
    font-weight: 600;
    cursor: pointer;
    font-size: 14px;
  }
  .url {
    flex: 1;
    max-width: 420px;
  }
  input {
    width: 100%;
    background: var(--gh-bg-inset);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
    color: var(--gh-fg);
    padding: var(--gh-space-1) var(--gh-space-2);
    font-size: 12px;
  }
  .spacer {
    flex: 1;
  }
  .ghost {
    background: transparent;
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
    color: var(--gh-fg-muted);
    cursor: pointer;
    padding: 2px 8px;
  }
</style>
