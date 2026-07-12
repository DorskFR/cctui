<script lang="ts">
  import { getContext } from "svelte";
  import { EMBED_THEME_KEY, type EmbedThemeContext } from "../embed/context";
  import { router } from "../router/router.svelte";
  import { currentTheme, setTheme, type Theme, THEME_LABELS, THEMES } from "../theme/theme";
  import SubscribeMenu from "./SubscribeMenu.svelte";

  const embedTheme = getContext<EmbedThemeContext | undefined>(EMBED_THEME_KEY);

  let theme = $state<Theme>(embedTheme ? embedTheme.get() : currentTheme());

  function onThemeChange(e: Event): void {
    theme = (e.currentTarget as HTMLSelectElement).value as Theme;
    if (embedTheme) embedTheme.set(theme);
    else setTheme(theme);
  }
</script>

<header class="topbar">
  <button class="brand" onclick={() => router.navigate("/")}>gh-review</button>
  <div class="spacer"></div>
  <SubscribeMenu />
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
