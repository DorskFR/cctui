<script lang="ts">
	import { Badge, Button, Card, Disclosure, IconButton, Link, Text } from '@dorsk/tsumikit';
	import type { FailedDispatch } from '@bindings/FailedDispatch';
	import { errMessage } from '$lib/api';
	import { sessionHref } from '$lib/sessionFailureToast';
	import { m } from '$lib/paraglide/messages';
	import { getLocale } from '$lib/paraglide/runtime';

	let {
		dispatch,
		onrerun,
		ondismiss
	}: {
		dispatch: FailedDispatch;
		onrerun: (sessionId: string) => Promise<unknown>;
		ondismiss?: (sessionId: string) => Promise<unknown>;
	} = $props();

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
		[dispatch.dispatcher, dispatch.profile, dispatch.repo, dispatch.model, dispatch.adapter]
			.filter(Boolean)
			.join(' · ')
	);
	const blocked = $derived.by(() => {
		switch (dispatch.rerun_blocked) {
			case 'env':
				return m.dispatch_failed_rerun_env({ keys: (dispatch.env_keys ?? []).join(', ') });
			case 'prompt_truncated':
				return m.dispatch_failed_rerun_prompt();
			case 'payload_too_large':
				return m.dispatch_failed_rerun_payload();
			default:
				return null;
		}
	});
	const at = $derived(new Date(dispatch.dispatched_at).toLocaleString(getLocale()));
	const supersededBy = $derived(dispatch.superseded_by ?? null);
	const exit = $derived(dispatch.worker_exit ?? null);
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

{#if supersededBy}
	<Card padding="sm" data-journey="failed-dispatch" data-resolved>
		<div class="row resolved">
			<div class="lead">
				<Badge tone="ok" size="sm" border>{m.dispatch_failed_resolved()}</Badge>
				<Text weight="bold" truncate>{title}</Text>
				<Text size="xs" tone="faint" truncate>{reason}</Text>
				<Text size="xs" tone="muted">
					{m.dispatch_failed_superseded_by()}
					<Link href={sessionHref(supersededBy)}>{supersededBy.slice(0, 8)}</Link>
				</Text>
			</div>
			{@render dismissButton()}
			{#if error}
				<Text size="sm" tone="danger">{error}</Text>
			{/if}
		</div>
	</Card>
{:else}
	<Card padding="sm" data-journey="failed-dispatch">
		<div class="row">
			<div class="lead">
				<Badge tone="danger" size="sm" border>{reason}</Badge>
				{#if dispatch.dismissed}
					<Badge tone="muted" size="sm">{m.dispatch_failed_dismissed()}</Badge>
				{/if}
				<Text weight="bold" truncate>{title}</Text>
				<Text size="xs" tone="faint" variant="code" truncate>{meta}</Text>
			</div>
			{@render dismissButton()}
			<div class="body">
				<Text size="xs" tone="faint">{m.dispatch_failed_dispatched_at({ at })}</Text>
				{#if dispatch.detail}
					<pre class="detail">{dispatch.detail}</pre>
				{/if}
				{#if exitLine}
					<Text size="sm" tone="muted">{exitLine}</Text>
				{/if}
				{#if exit?.log_tail}
					<Disclosure open size="compact" chevron="start">
						{#snippet header()}
							<Text size="xs" tone="faint">{m.dispatch_failed_log_tail()}</Text>
						{/snippet}
						<pre class="log">{exit.log_tail}</pre>
					</Disclosure>
				{:else if exit?.logs_error}
					<Text size="xs" tone="faint">{m.dispatch_failed_logs_unavailable({ error: exit.logs_error })}</Text>
				{/if}
				{#if dispatch.prompt_head}
					<pre class="prompt">{dispatch.prompt_head}</pre>
				{:else if dispatch.prompt_file}
					<Text size="sm" tone="muted">{m.dispatch_failed_prompt_file({ file: dispatch.prompt_file })}</Text>
				{/if}
				{#if blocked}
					<Text size="sm" tone="muted">{blocked}</Text>
				{:else}
					<Text size="xs" tone="faint">{m.dispatch_failed_rerun_note()}</Text>
				{/if}
				{#if error}
					<Text size="sm" tone="danger">{error}</Text>
				{/if}
			</div>
			{#if !blocked}
				<Button size="sm" variant="ghost" disabled={busy} onclick={() => run(onrerun)}>
					{m.dispatch_failed_rerun()}
				</Button>
			{/if}
		</div>
	</Card>
{/if}

<style>
	.row {
		display: flex;
		align-items: flex-start;
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
	.body {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		flex: 1 1 100%;
		min-width: 0;
	}
	pre {
		margin: 0;
		max-height: 8rem;
		overflow: auto;
		white-space: pre-wrap;
		font: var(--font-mono, monospace);
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
	.resolved {
		align-items: center;
	}
</style>
