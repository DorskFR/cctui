<script lang="ts">
  import { onDestroy, setContext } from "svelte";
  import "./embed.css";
  import { configureRuntime } from "./lib/api/config";
  import Shell from "./lib/components/Shell.svelte";
  import { EMBED_KEY, type EmbedContext } from "./lib/embed/context";
  import { router } from "./lib/router/router.svelte";

  interface Props {
    baseUrl: string;
    token: string | null;
    account?: string | null;
    basePath?: string;
  }
  let { baseUrl, token, account = null, basePath = "" }: Props = $props();

  // pre-effects run ahead of the child query-subscription effects, so the
  // backend URL + token are in place before Shell issues its first request.
  $effect.pre(() => {
    configureRuntime({ baseUrl, token, account, basePath });
    router.refresh();
  });

  onDestroy(() => configureRuntime(null));

  setContext<EmbedContext>(EMBED_KEY, { embedded: true });
</script>

<div class="ghreview-embed">
  <Shell />
</div>
