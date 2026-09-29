<script lang="ts">
  import type { Component } from "svelte";
  import { setContext } from "svelte";
  import { HOST_CONTEXT_KEY, type HostContext, type PageProps } from "../plugin/host";

  interface Props {
    host: HostContext;
    component: Component<PageProps>;
    page: PageProps;
  }
  let { host, component, page }: Props = $props();

  setContext<HostContext>(HOST_CONTEXT_KEY, host);

  const Page = $derived(component);
</script>

<Page {...page} />
