<script lang="ts">
  import { rewriteGithubUrl } from "../bookmarklet";
  import { router } from "../router/router.svelte";
  import { currentTheme, setTheme, type Theme, THEME_LABELS, THEMES } from "../theme/theme";

  let prUrl = $state("");
  let theme = $state<Theme>(currentTheme());

  function openUrl(e: SubmitEvent): void {
    e.preventDefault();
    const target = rewriteGithubUrl(prUrl.trim(), window.location.origin);
    if (target) {
      router.navigate(new URL(target).pathname);
      prUrl = "";
    }
  }

  function onThemeChange(e: Event): void {
    theme = (e.currentTarget as HTMLSelectElement).value as Theme;
    setTheme(theme);
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
  <select class="theme" value={theme} onchange={onThemeChange} title="Theme" aria-label="Theme">
    {#each THEMES as t (t)}
      <option value={t}>{THEME_LABELS[t]}</option>
    {/each}
  </select>
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
  .theme {
    background: var(--gh-bg-inset);
    border: 1px solid var(--gh-border);
    border-radius: var(--gh-radius);
    color: var(--gh-fg-muted);
    cursor: pointer;
    padding: 2px 8px;
    font-family: inherit;
    font-size: 12px;
  }
</style>
