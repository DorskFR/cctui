<!--
  `/github` (+ sub-paths) — the unified GitHub review center (CCT-674). Replaces
  the old Rust-backed GithubView; the review UI now lives here, backed by the
  ghreview TS backend and mounted lazily so its chunk loads only on this route.

  Three states:
    • `ghreviewUrl` unset          → "not configured" panel (graceful degrade).
    • configured but no connector  → unlock screen pointing at Accounts.
    • configured + connector       → the embedded Review app (basePath `/github`).

  Account plumb-through (CCT-674): we fetch the caller's ghreview accounts and
  pass one into <Review account=…/> so getAccount() is non-null and the repo
  picker (`GET /v1/github/repos?account=…`) works.
-->
<script lang="ts">
	import { ghreviewUrl } from '$lib/config';
	import { ensureGhreviewToken } from '$lib/ghreview';
	import { useCapabilities } from '$lib/queries';
	import { Card, Field, Heading, Link, Select, Stack, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	const url = ghreviewUrl();
	const caps = useCapabilities();
	const enabled = $derived($caps.data?.github.enabled === true);

	interface GhAccount {
		id: string;
		login: string;
	}

	let accounts = $state<GhAccount[]>([]);
	let account = $state<string | null>(null);

	async function boot(base: string) {
		const [mod, token] = await Promise.all([
			import('$ghreview/Review.svelte'),
			ensureGhreviewToken()
		]);
		try {
			const res = await fetch(`${base}/v1/accounts`, {
				headers: { authorization: `Bearer ${token}` }
			});
			if (res.ok) {
				const body = (await res.json()) as { items?: GhAccount[] };
				accounts = body.items ?? [];
			}
		} catch {
			accounts = [];
		}
		if (accounts.length > 0) account = accounts[0].login;
		return { Review: mod.default, token, base };
	}

	// url is a constant deploy value, so kick off the boot once the connector gate
	// resolves. An $effect (not a $derived) so the account/accounts writes above
	// aren't side-effects of a derivation.
	let booted = $state<ReturnType<typeof boot> | null>(null);
	$effect(() => {
		if (url && enabled && !booted) booted = boot(url);
	});

</script>

{#if !url}
	<Card>
		<Stack gap="var(--sp-2)">
			<Heading level={2}>{m.review_center_not_configured()}</Heading>
			<Text tone="faint">
				{m.review_config_hint_prefix()} <code>ghreviewUrl</code> {m.review_config_hint_suffix()}
			</Text>
		</Stack>
	</Card>
{:else if $caps.isSuccess && !enabled}
	<Card>
		<Stack gap="var(--sp-2)">
			<Heading level={2}>{m.github_unlock_heading()}</Heading>
			<Text tone="faint">{m.github_unlock_body()}</Text>
			<Link href="/accounts">{m.github_unlock_cta()}</Link>
		</Stack>
	</Card>
{:else if enabled}
	{#if !booted}
		<Text tone="faint">{m.review_center_loading()}</Text>
	{:else}
		{#await booted}
			<Text tone="faint">{m.review_center_loading()}</Text>
		{:then { Review, token, base }}
			{#if accounts.length === 0}
				<Card>
					<Stack gap="var(--sp-2)">
						<Heading level={2}>{m.github_no_review_accounts_heading()}</Heading>
						<Text tone="faint">{m.github_no_review_accounts_body()}</Text>
						<Link href="/accounts">{m.github_unlock_cta()}</Link>
					</Stack>
				</Card>
			{:else}
				{#if accounts.length > 1}
					<Field label={m.github_account_label()}>
						<Select bind:value={account}>
							{#each accounts as a (a.id)}
								<option value={a.login}>{a.login}</option>
							{/each}
						</Select>
					</Field>
				{/if}
				<Review baseUrl={base} {token} {account} basePath="/github" />
			{/if}
		{:catch}
			<Card>
				<Stack gap="var(--sp-2)">
					<Heading level={2}>{m.review_center_unavailable()}</Heading>
					<Text tone="faint">{m.review_center_unreachable()}</Text>
				</Stack>
			</Card>
		{/await}
	{/if}
{/if}
