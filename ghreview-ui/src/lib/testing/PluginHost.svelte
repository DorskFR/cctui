<script lang="ts">
  import type { Component } from "svelte";
  import { setContext } from "svelte";
  import { HOST_CONTEXT_KEY, type HostContext } from "../plugin/host";

  interface Props {
    host: HostContext;
    component: Component<never>;
    props?: Record<string, unknown>;
  }
  let { host, component, props = {} }: Props = $props();

  setContext<HostContext>(HOST_CONTEXT_KEY, host);

  const Hosted = $derived(component as unknown as Component<Record<string, unknown>>);
</script>

<Hosted {...props} />
