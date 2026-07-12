<!--
  `/review` (+ sub-paths) — the gh-review connector (CCT-610). The review UI is a
  workspace dependency mounted lazily so its chunk loads only here, and only when
  `ghreviewUrl` is configured. Unset → a "not configured" panel, never a broken
  route or console noise (graceful degradation). The router runs under a
  `/review` base path; gh-review's own GitHub-mirrored paths stay intact.
-->
<script lang="ts">
	import { ghreviewUrl } from '$lib/config';
	import { ensureGhreviewToken } from '$lib/ghreview';
	import { Card, Heading, Stack, Text } from '@dorsk/tsumikit';

	const url = ghreviewUrl();

	async function boot(base: string) {
		const [mod, token] = await Promise.all([
			import('$ghreview/Review.svelte'),
			ensureGhreviewToken()
		]);
		return { Review: mod.default, token, base };
	}

	const booted = url ? boot(url) : null;
</script>

{#if !booted}
	<Card>
		<Stack gap="var(--sp-2)">
			<Heading level={2}>Review center not configured</Heading>
			<Text tone="faint">
				Set <code>ghreviewUrl</code> in the deployment config to enable the gh-review integration.
			</Text>
		</Stack>
	</Card>
{:else}
	{#await booted}
		<Text tone="faint">Loading review center…</Text>
	{:then { Review, token, base }}
		<Review baseUrl={base} {token} basePath="/review" />
	{:catch}
		<Card>
			<Stack gap="var(--sp-2)">
				<Heading level={2}>Review center unavailable</Heading>
				<Text tone="faint">Could not reach the gh-review backend or mint an access token.</Text>
			</Stack>
		</Card>
	{/await}
{/if}
