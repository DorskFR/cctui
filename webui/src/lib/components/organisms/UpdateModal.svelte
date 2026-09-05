<script lang="ts">
	// "A newer cctui is out" — opened from the red ↑ chip in the header.
	// Step 1 shows the release notes the server's probe collected (no GitHub
	// call from the browser) and, for admins, the red "Update" button; other
	// users read "ask your administrator". Step 2 is the plain-language
	// consent: a YOLO agent takes over, a few minutes, a short outage. Yes
	// hands the upgrade to `POST /version/self-update` and jumps to the
	// session it spawned.
	import { goto } from '$app/navigation';
	import { Button, Modal, Text } from '@dorsk/tsumikit';
	import { createQuery } from '@tanstack/svelte-query';
	import { endpoints, useMe } from '$lib/queries';
	import { renderMarkdown } from '$lib/markdown';
	import { toasts } from '$lib/toast.svelte';
	import { ws } from '$lib/ws.svelte';
	import { m } from '$lib/paraglide/messages';

	let {
		latestVersion,
		latestUrl,
		selfUpdateReady,
		onclose
	}: {
		latestVersion: string;
		latestUrl: string;
		/** `VersionInfo.self_update_ready`: a self-update machine is configured. */
		selfUpdateReady: boolean;
		onclose: () => void;
	} = $props();

	const me = useMe();
	const isAdmin = $derived(me.data?.role === 'admin');
	const changelog = createQuery(() => ({
		queryKey: ['version', 'changelog', latestVersion],
		queryFn: endpoints.changelog,
		staleTime: 60_000
	}));

	let step = $state<'notes' | 'confirm'>('notes');
	let launching = $state(false);

	async function launch() {
		launching = true;
		try {
			const res = await endpoints.selfUpdate();
			toasts.info(m.update_toast_launched({ version: res.version }));
			const ack = await ws.awaitCommand(String(res.command_id));
			if (ack.ok || ack.timedOut) {
				onclose();
				await goto(res.session_id ? `/sessions/${encodeURIComponent(res.session_id)}` : '/sessions');
			} else {
				toasts.error(m.update_toast_failed({ error: ack.error ?? m.spawn_error_unknown() }));
			}
		} catch (e) {
			toasts.error(m.update_toast_failed({ error: e instanceof Error ? e.message : String(e) }));
		} finally {
			launching = false;
		}
	}
</script>

{#if step === 'notes'}
	<Modal title={m.update_modal_title({ version: latestVersion })} {onclose} size="lg" {body} {footer} />
{:else}
	<Modal title={m.update_confirm_title()} {onclose} size="sm" body={confirmBody} footer={confirmFooter} />
{/if}

{#snippet body()}
	<div class="notes">
		{#if changelog.isPending}
			<Text tone="faint" size="sm">{m.update_changelog_loading()}</Text>
		{:else if changelog.isError}
			<Text tone="danger" size="sm">{m.update_changelog_failed()}</Text>
		{:else if !changelog.data?.releases.length}
			<Text tone="faint" size="sm">{m.update_changelog_empty()}</Text>
		{:else}
			{#each changelog.data.releases as rel (rel.version)}
				<section class="release">
					<Text as="div" weight="bold" variant="code">
						<a class="rel-link" href={rel.url} target="_blank" rel="noopener">v{rel.version}</a>
					</Text>
					{#if rel.body}
						<div class="md">{@html renderMarkdown(rel.body)}</div>
					{:else}
						<Text tone="faint" size="sm">{m.update_release_no_notes()}</Text>
					{/if}
				</section>
			{/each}
		{/if}
		<Text as="div" size="xs" tone="faint">
			<a class="rel-link" href={latestUrl} target="_blank" rel="noopener">{m.update_open_release_page()}</a>
		</Text>
	</div>
{/snippet}

{#snippet footer()}
	{#if isAdmin}
		{#if !selfUpdateReady}
			<Text size="sm" tone="faint">{m.update_no_target_hint()}</Text>
		{/if}
		<Button size="lg" onclick={onclose}>{m.update_not_now()}</Button>
		<Button size="lg" variant="danger" disabled={!selfUpdateReady} onclick={() => (step = 'confirm')}>
			{m.update_button()}
		</Button>
	{:else}
		<Text size="sm" tone="faint">{m.update_ask_admin()}</Text>
		<Button size="lg" onclick={onclose}>{m.update_not_now()}</Button>
	{/if}
{/snippet}

{#snippet confirmBody()}
	<Text as="p">{m.update_confirm_body()}</Text>
{/snippet}

{#snippet confirmFooter()}
	<Button size="lg" disabled={launching} onclick={onclose}>{m.update_confirm_no()}</Button>
	<Button size="lg" variant="danger" loading={launching} disabled={launching} onclick={launch}>
		{m.update_confirm_yes()}
	</Button>
{/snippet}

<style>
	.notes {
		display: flex;
		flex-direction: column;
		gap: var(--sp-4);
		max-height: 60vh;
		overflow: auto;
	}
	.release {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding-bottom: var(--sp-3);
		border-bottom: 1px solid var(--border);
	}
	.release:last-of-type {
		border-bottom: 0;
	}
	.rel-link {
		color: inherit;
	}
	.md {
		font-size: var(--fs-sm);
		line-height: var(--lh-normal);
		overflow-wrap: anywhere;
	}
</style>
