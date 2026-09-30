<script lang="ts">
  import { Button, Callout, EmptyState, Stack, Text } from "@dorsk/tsumikit";
  import { onDestroy, onMount, setContext } from "svelte";
  import "./embed.css";
  import { configureRuntime } from "./lib/api/config";
  import Shell from "./lib/components/Shell.svelte";
  import { EMBED_KEY, type EmbedContext } from "./lib/embed/context";
  import { backendPath, hostContext, type PageProps } from "./lib/plugin/host";
  import { router } from "./lib/router/router.svelte";

  let { basePath, path, navigate }: PageProps = $props();

  const host = hostContext();
  const pluginFetch = host?.pluginFetch;

  type Probe = "checking" | "ready" | "unavailable";
  let probe = $state<Probe>("checking");
  let detail = $state("");

  // configureRuntime must land before Shell's children subscribe their queries,
  // so it runs in a pre-effect rather than onMount.
  $effect.pre(() => {
    if (!pluginFetch) return;
    configureRuntime({
      basePath,
      transport: {
        fetch: (p, init) => pluginFetch(p, init),
        eventsUrl: () => backendPath("/v1/events"),
      },
    });
  });

  $effect.pre(() => {
    if (!pluginFetch) return;
    router.adopt({ navigate });
    router.setPath(path);
  });

  async function check(): Promise<void> {
    if (!pluginFetch) return;
    probe = "checking";
    detail = "";
    try {
      const res = await pluginFetch("/v1/health");
      if (res.ok) {
        probe = "ready";
        return;
      }
      probe = "unavailable";
      detail = `The proxy answered ${res.status} ${res.statusText}.`;
    } catch (error) {
      probe = "unavailable";
      detail = error instanceof Error ? error.message : String(error);
    }
  }

  onMount(check);

  onDestroy(() => {
    router.adopt(null);
    configureRuntime(null);
  });

  setContext<EmbedContext>(EMBED_KEY, { embedded: true });
</script>

<div class="ghreview-embed">
  {#if !pluginFetch}
    <div class="gate">
      <Callout tone="danger" title="This cctui is too old for the review plugin">
        The host does not expose <code>pluginFetch</code>, so the plugin cannot reach its backend.
        Update cctui to a build that speaks plugin API 1.1.
      </Callout>
    </div>
  {:else if probe === "checking"}
    <EmptyState loading title="Contacting the review backend…" />
  {:else if probe === "unavailable"}
    <div class="gate">
      <Stack gap="var(--gh-space-3)">
        <EmptyState
          icon="unlink"
          title="The review backend is not configured or not reachable"
          description="An administrator sets this plugin's backend URL in Settings › Plugins. Until then, and whenever the backend is down, the review centre has nothing to read."
        />
        {#if detail}
          <Text size="sm" tone="muted">{detail}</Text>
        {/if}
        <div class="retry">
          <Button variant="primary" onclick={check}>Retry</Button>
        </div>
      </Stack>
    </div>
  {:else}
    <Shell />
  {/if}
</div>

<style>
  .gate {
    flex: 1;
    display: grid;
    place-items: center;
    padding: var(--gh-space-4);
  }
  .retry {
    display: flex;
    justify-content: center;
  }
  code {
    font-family: var(--gh-mono);
  }
</style>
