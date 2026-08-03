<script lang="ts">
  import { type QueryClient, QueryClientProvider } from "@tanstack/svelte-query";
  import type { Component } from "svelte";

  interface Props {
    client: QueryClient;
    component: Component<never>;
    props?: Record<string, unknown>;
  }
  let { client, component, props = {} }: Props = $props();
  const Child = $derived(component as unknown as Component<Record<string, unknown>>);
</script>

<QueryClientProvider {client}>
  <Child {...props} />
</QueryClientProvider>
