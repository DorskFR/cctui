<script lang="ts">
	import { Badge, Button, Disclosure, IconButton, Link, Text, Timestamp } from '@dorsk/tsumikit';
	import type { FailedDispatch } from '@bindings/FailedDispatch';
	import { errMessage } from '$lib/api';
	import { exposureView } from '$lib/dispatchExposure';
	import { sessionHref } from '$lib/sessionFailureToast';
	import { m } from '$lib/paraglide/messages';
	import { getLocale } from '$lib/paraglide/runtime';
	import {
		DISPATCHERS_HREF,
		blockedExplanation,
		promptFileHref,
		reasonLabel
	} from '../../../routes/dispatch-failures/causes';

	let {
		dispatch,
		onrerun,
		ondismiss,
		showCause = true
	}: {
		dispatch: FailedDispatch;
		onrerun: (sessionId: string) => Promise<unknown>;
		ondismiss?: (sessionId: string) => Promise<unknown>;
		showCause?: boolean;
	} = $props();

	let busy = $state(false);
	let error = $state<string | null>(null);

	const reason = $derived(reasonLabel(dispatch.reason));
	const title = $derived(
		dispatch.name?.trim() || dispatch.prompt_file || dispatch.session_id.slice(0, 8)
	);
	const meta = $derived(
		[dispatch.profile, dispatch.repo, dispatch.model, dispatch.adapter].filter(Boolean).join(' · ')
	);
	const blocked = $derived(blockedExplanation(dispatch));
	const promptHref = $derived(
		dispatch.prompt_file ? promptFileHref(dispatch.repo, dispatch.prompt_file) : null
	);
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

<div class="fd" data-journey="failed-dispatch" data-resolved={supersededBy ? '' : undefined}>
	<div class="line">
		{#if supersededBy}
			{#if showCause}<Badge tone="ok" size="sm">{m.dispatch_failed_resolved()}</Badge>{/if}
		{:else if showCause}
			<Badge tone="danger" size="sm">{reason}</Badge>
		{/if}
		<span class="title"><Text weight="semibold" truncate>{title}</Text></span>
		<Timestamp value={dispatch.dispatched_at} mode="relative" size="xs" tone="faint" />
		{#if dispatch.dismissed}
			<Text size="xs" tone="faint">{m.dispatch_failed_dismissed()}</Text>
		{/if}
		{#if dispatch.prompt_file}
			<span class="file" title={m.dispatch_failed_prompt_file({ file: dispatch.prompt_file })}>
				{#if promptHref}
					<Link href={promptHref} target="_blank" rel="noopener noreferrer"
						><Text size="xs" mono truncate>{dispatch.prompt_file}</Text></Link
					>
				{:else}
					<Text size="xs" mono tone="muted" truncate>{dispatch.prompt_file}</Text>
				{/if}
			</span>
		{/if}
		<Link href={DISPATCHERS_HREF} tone="muted" data-dispatcher
			><Text size="xs" tone="inherit">{dispatch.dispatcher}</Text></Link
		>
		<span class="spacer"></span>
		{#if !blocked && !supersededBy}
			<Button size="sm" disabled={busy} onclick={() => run(onrerun)}>
				{m.dispatch_failed_rerun()}
			</Button>
		{/if}
		{#if ondismiss}
			<IconButton
				icon="x"
				variant="ghost"
				box="sm"
				label={m.dispatch_failed_dismiss()}
				title={m.dispatch_failed_dismiss()}
				disabled={busy}
				onclick={() => ondismiss && run(ondismiss)}
			/>
		{/if}
	</div>
	{#if supersededBy}
		<Text size="xs" tone="muted">
			{reason} ·
			{m.dispatch_failed_superseded_by()}
			<Link href={sessionHref(supersededBy)}>{supersededBy.slice(0, 8)}</Link>
		</Text>
	{:else if showCause && blocked}
		<Text size="sm" tone="muted">{blocked}</Text>
	{/if}
	{#if error}
		<Text size="sm" tone="danger">{error}</Text>
	{/if}
	{#if !supersededBy}
		<Disclosure size="compact" chevron="start">
			{#snippet header()}
				<Text size="xs" tone="faint">{m.list_failed_details()}</Text>
			{/snippet}
			<div class="body">
				<Text size="xs" tone="faint">{m.dispatch_failed_dispatched_at({ at })}</Text>
				{#if meta}
					<Text size="xs" tone="muted" data-meta>{meta}</Text>
				{/if}
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
				{#if !blocked}
					<Text size="xs" tone="faint">{m.dispatch_failed_rerun_note()}</Text>
				{/if}
			</div>
		</Disclosure>
	{/if}
</div>

<style>
	.fd {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		min-width: 0;
	}
	.line {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: var(--sp-1) var(--sp-2);
		min-width: 0;
	}
	.title {
		display: inline-flex;
		min-width: 0;
		flex: 0 1 auto;
	}
	.file {
		display: inline-flex;
		min-width: 0;
		max-width: 24rem;
	}
	.spacer {
		flex: 1;
	}
	.body {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		padding-block: var(--sp-1);
		min-width: 0;
	}
	.exposure {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: var(--sp-1);
	}
	pre {
		margin: 0;
		max-height: 8rem;
		overflow: auto;
		white-space: pre-wrap;
		font-family: var(--font-mono);
		font-size: var(--fs-xs);
		user-select: text;
	}
	.detail {
		color: var(--danger);
	}
	.prompt,
	.log {
		color: var(--text-muted);
	}
	.log {
		max-height: 16rem;
	}
</style>
