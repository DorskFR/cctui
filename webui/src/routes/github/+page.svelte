<!--
  `/github` route (CCT-375 / GH-CAP-1) — capability-gated + lazy-loaded.

  The actual view (`GithubView.svelte`, later a heavy diff viewer) is pulled in
  via a dynamic `import()`, so its chunk is fetched only when this route is
  visited — non-GitHub users never download it. Access is gated on the server
  capability: while it resolves we show nothing; if GitHub is disabled we send
  the user home (the nav item is hidden, so reaching here means a stale/manual
  URL).
-->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { useCapabilities } from '$lib/queries';

	const caps = useCapabilities();

	// Lazy chunk: only loaded when the route renders.
	const view = import('$lib/components/organisms/GithubView.svelte');

	$effect(() => {
		if ($caps.isSuccess && !$caps.data.github.enabled) {
			goto('/', { replaceState: true });
		}
	});
</script>

{#if $caps.data?.github.enabled}
	{#await view then m}
		<m.default />
	{/await}
{/if}
