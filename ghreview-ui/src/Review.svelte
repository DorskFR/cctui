<script lang="ts">
  import { setContext } from "svelte";
  import "./embed.css";
  import { configureRuntime } from "./lib/api/config";
  import { CanvasDiffRenderer } from "./lib/components/CanvasDiffRenderer";
  import { DomDiffRenderer } from "./lib/components/DomDiffRenderer";
  import Shell from "./lib/components/Shell.svelte";
  import { EMBED_THEME_KEY, type EmbedThemeContext } from "./lib/embed/context";
  import { registerRenderer } from "./lib/diff/renderer";
  import { getStoredTheme, storeTheme, type Theme } from "./lib/theme/theme";
  import { router } from "./lib/router/router.svelte";

  interface Props {
    baseUrl: string;
    token: string | null;
    account?: string | null;
    basePath?: string;
  }
  let { baseUrl, token, account = null, basePath = "" }: Props = $props();

  registerRenderer(DomDiffRenderer);
  registerRenderer(CanvasDiffRenderer);

  // pre-effects run ahead of the child query-subscription effects, so the
  // backend URL + token are in place before Shell issues its first request.
  $effect.pre(() => {
    configureRuntime({ baseUrl, token, account, basePath });
    router.refresh();
  });

  let theme = $state<Theme>(getStoredTheme() ?? "dark");
  setContext<EmbedThemeContext>(EMBED_THEME_KEY, {
    get: () => theme,
    set: (t) => {
      theme = t;
      storeTheme(t);
    },
  });
</script>

<div class="ghreview-embed" data-theme={theme}>
  <Shell />
</div>
