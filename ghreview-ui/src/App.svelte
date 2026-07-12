<script lang="ts">
  import { getToken } from "./lib/api/config";
  import AuthGate from "./lib/components/AuthGate.svelte";
  import Shell from "./lib/components/Shell.svelte";

  let hasToken = $state(!!getToken());
</script>

{#if !hasToken}
  <AuthGate onauthed={() => (hasToken = true)} />
{:else}
  <Shell />
{/if}
