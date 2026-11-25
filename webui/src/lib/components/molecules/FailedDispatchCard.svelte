<script lang="ts">
	import { untrack } from 'svelte';
	import { Badge, Button, Card, Disclosure, IconButton, Link, Text } from '@dorsk/tsumikit';
	import type { FailedDispatch } from '@bindings/FailedDispatch';
	import { errMessage } from '#lib/api.ts';
	import { exposureView } from '#lib/dispatchExposure.ts';
	import { sessionHref } from '#lib/sessionFailureToast.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { getLocale } from '#lib/paraglide/runtime.js';

	let {
		dispatch,
		dispatcherHref = null,
		expanded = false,
		onrerun,
		ondismiss
	}: {
		dispatch: FailedDispatch;
		/** Where the dispatcher is managed, when it is one of the viewer's. */
		dispatcherHref?: string | null;
		expanded?: boolean;
		onrerun: (sessionId: string) => Promise<unknown>;
		ondismiss?: (sessionId: string) => Promise<unknown>;
	} = $props();

	let open = $state(untrack(() => expanded));

	let busy = $state(false);
	let error = $state<string | null>(null);

	const reason = $derived(
		{
			refused: m.dispatch_failed_refused(),
			never_registered: m.dispatch_failed_never_registered(),
			oom_killed: m.sessions_end_oom_killed(),
			crashed: m.sessions_end_crashed(),
			spawn_failed: m.sessions_end_spawn_failed()
		}[dispatch.reason]
	);
	const title = $derived(
		dispatch.name?.trim() || dispatch.prompt_file || dispatch.session_id.slice(0, 8)
	);
	const meta = $derived(
		[dispatch.profile, dispatch.repo, dispatch.model, dispatch.adapter].filter(Boolean).join(' · ')
	);
	const metaSuffix = $derived(meta ? ` · ${meta}` : '');
	const promptFile = $derived(dispatch.prompt_file && dispatch.prompt_file !== title ? dispatch.prompt_file : null);
	const blocked = $derived(!!dispatch.rerun_blocked);
	const at = $derived(new Date(dispatch.dispatched_at).toLocaleString(getLocale()));
	const supersededBy = $derived(dispatch.superseded_by ?? null);
	const exit = $derived(dispatch.worker_exit ?? null);
	const exposure = $derived(exposureView(dispatch.exposure));
	const exitLine = $derived.by(() => {
		if (exit?.exit_code == null) return null;
		const line = m.dispatch_failed_exit({
			container: exit.container ?? '',
			code: String(exit.exit_code)
		});
		const signal =
			exit.signal != null ? m.dispatch_failed_signal({ signal: String(exit.signal) }) : null;
		const why = [exit.reason, signal].filter(Boolean).join(' · ');
		return why ? `${line} (${why})` : line;
	});

	async function run(action: (sessionId: string) => Promise<unknown>) {
		busy = true;
		error = null;
		try {
			await action(dispatch.session_id);
		} catch (e) {
			error = errMessage(e);
		} finally {
			busy = false;
		}
	}
</script>

{#snippet dismissButton()}
	{#if ondismiss}
		<IconButton
			icon="x"
			label={m.dispatch_failed_dismiss()}
			inline
			size={13}
			disabled={busy}
			onclick={() => ondismiss && run(ondismiss)}
		/>
	{/if}
{/snippet}

{#snippet when()}
	<Text as="span" size="xs" tone="faint">
		<time datetime={dispatch.dispatched_at}>{at}</time>
	</Text>
{/snippet}

{#if supersededBy}
	<Card padding="sm" data-journey="failed-dispatch" data-resolved>
		<div class="row">
			<div class="lead">
				<Text weight="semibold" truncate>{title}</Text>
				{@render when()}
				<Text size="xs" tone="faint" truncate>{reason}</Text>
				<Text size="xs" tone="muted">
					{m.dispatch_failed_superseded_by()}
					<Link href={sessionHref(supersededBy)}>{supersededBy.slice(0, 8)}</Link>
				</Text>
			</div>
			{@render dismissButton()}
		</div>
		{#if error}
			<Text size="sm" tone="danger">{error}</Text>
		{/if}
	</Card>
{:else}
	<Card padding="sm" data-journey="failed-dispatch">
		<div class="row">
			<div class="lead">
				<Badge tone="danger" size="sm">{reason}</Badge>
				{#if dispatch.dismissed}
					<Badge tone="muted" size="sm">{m.dispatch_failed_dismissed()}</Badge>
				{/if}
				<Text weight="semibold" truncate>{title}</Text>
				{@render when()}
				{#if promptFile}
					<Text as="span" size="xs" tone="muted" variant="code" truncate>{promptFile}</Text>
				{/if}
			</div>
			{#if !blocked}
				<Button size="sm" variant="ghost" disabled={busy} onclick={() => run(onrerun)}>
					{m.dispatch_failed_rerun()}
				</Button>
			{/if}
			{@render dismissButton()}
		</div>
		{#if error}
			<Text size="sm" tone="danger">{error}</Text>
		{/if}
		<Disclosure bind:open size="compact" chevron="start">
			{#snippet header()}
				<Text size="xs" tone="faint">{m.dispatch_failed_details()}</Text>
			{/snippet}
			<div class="body">
				<Text size="xs" tone="faint">
					{#if dispatcherHref}
						<Link href={dispatcherHref}>{dispatch.dispatcher}</Link>
					{:else}
						{dispatch.dispatcher}
					{/if}{metaSuffix}
				</Text>
				{#if exposure}
					<div class="exposure" data-exposure>
						<Text size="xs" tone="faint">{m.exposure_label()}</Text>
						{#each exposure.flags as flag (flag.key)}
							<Badge
								size="xs"
								tone={flag.on ? 'warn' : 'muted'}
								removed={!flag.on}
								data-flag={flag.key}
								data-on={String(flag.on)}
							>
								{flag.label}
							</Badge>
						{/each}
						{#if exposure.gate}
							<Badge size="xs" tone="ok" data-gate>{exposure.gate}</Badge>
						{:else if exposure.ungated}
							<Badge size="xs" tone="danger" data-gate>{m.exposure_no_gate()}</Badge>
						{/if}
					</div>
				{/if}
				{#if dispatch.detail}
					<pre class="detail">{dispatch.detail}</pre>
				{/if}
				{#if exitLine}
					<Text size="sm" tone="muted">{exitLine}</Text>
				{/if}
				{#if exit?.log_tail}
					<Text size="xs" tone="faint">{m.dispatch_failed_log_tail()}</Text>
					<pre class="log">{exit.log_tail}</pre>
				{:else if exit?.logs_error}
					<Text size="xs" tone="faint">{m.dispatch_failed_logs_unavailable({ error: exit.logs_error })}</Text>
				{/if}
				{#if dispatch.prompt_head}
					<pre class="prompt">{dispatch.prompt_head}</pre>
				{/if}
			</div>
		</Disclosure>
	</Card>
{/if}

<style>
	.row {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		flex-wrap: wrap;
	}
	.lead {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
		flex: 1 1 16rem;
	}
	.exposure {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: var(--sp-1);
	}
	.body {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		min-width: 0;
	}
	pre {
		margin: 0;
		max-height: 8rem;
		overflow: auto;
		white-space: pre-wrap;
		font-family: var(--font-mono, monospace);
		font-size: var(--fs-xs, 0.75rem);
		user-select: text;
	}
	.detail {
		color: var(--danger);
	}
	.prompt {
		color: var(--text-muted);
	}
	.log {
		max-height: 16rem;
		color: var(--text-muted);
	}
</style>
