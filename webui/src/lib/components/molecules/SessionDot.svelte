<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { Tooltip, copyToClipboard } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { sessionDebugRows } from '../../../routes/sessions/sessions.logic';

	// Activity dot: the liveness dot carries a rich debug tooltip —
	// session id (surfaced nowhere else, click-to-copy) plus account, created,
	// machine, keepalive, credentials and status. `livenessClass` and `now` are
	// derived by the caller (SessionCard / DrawerHeader) so the dot color and the
	// stale/relative-age words stay in sync with the row.
	let {
		session,
		livenessClass,
		now = Date.now()
	}: { session: SessionListItem; livenessClass: string; now?: number } = $props();

	const rows = $derived(sessionDebugRows(session, now));

	let copied = $state(false);
	let timer: ReturnType<typeof setTimeout> | undefined;
	async function copyId() {
		const ok = await copyToClipboard(session.id);
		copied = ok;
		clearTimeout(timer);
		timer = setTimeout(() => (copied = false), 1200);
	}
</script>

<Tooltip>
	{#snippet trigger()}
		<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
		<span
			class="dot {livenessClass}"
			role="img"
			tabindex="0"
			aria-label={m.sessions_dot_aria()}
		></span>
	{/snippet}
	{#snippet content()}
		<div class="dbg">
			<div class="idrow">
				<code class="id">{session.id}</code>
				<button type="button" class="copy" onclick={copyId} title={m.sessions_copy_id_title()}>
					{copied ? '✓' : '⧉'}
				</button>
			</div>
			<dl class="grid">
				{#each rows as r (r.label)}
					<dt>{r.label}</dt>
					<dd>{r.value}</dd>
				{/each}
			</dl>
		</div>
	{/snippet}
</Tooltip>

<style>
	.dbg {
		font-size: var(--fs-xs);
		line-height: 1.4;
	}
	.idrow {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		margin-bottom: var(--sp-2);
	}
	.id {
		font-family: var(--font-mono, monospace);
		user-select: all;
		word-break: break-all;
		color: var(--text);
	}
	.copy {
		flex: none;
		background: none;
		border: 1px solid var(--border-strong);
		border-radius: var(--r-sm);
		color: var(--text-muted);
		cursor: pointer;
		padding: 0 var(--sp-1);
		line-height: 1.4;
	}
	.copy:hover {
		color: var(--text);
	}
	.grid {
		display: grid;
		grid-template-columns: auto 1fr;
		column-gap: var(--sp-2);
		row-gap: 0.15rem;
		margin: 0;
	}
	.grid dt {
		color: var(--text-faint);
		font-family: var(--font-mono, monospace);
	}
	.grid dd {
		margin: 0;
		font-family: var(--font-mono, monospace);
		word-break: break-word;
		color: var(--text);
	}
</style>
