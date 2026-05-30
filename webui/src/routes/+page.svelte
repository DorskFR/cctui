<script lang="ts">
	import { useUsers, useSessions } from '$lib/queries';
	import { apiOrigin } from '$lib/config';
	import { toasts } from '$lib/toast.svelte';
	import { auth } from '$lib/auth.svelte';

	const users = useUsers();
	const sessions = useSessions(() => true);

	const activeUsers = $derived(($users.data ?? []).filter((u) => !u.revoked_at).length);
	const revokedUsers = $derived(($users.data ?? []).filter((u) => u.revoked_at).length);
	const all = $derived($sessions.data?.sessions ?? []);
	const live = $derived(all.filter((s) => s.status === 'active' || s.status === 'new').length);
	const archived = $derived(all.filter((s) => s.status === 'archived').length);
	const needs = $derived(all.filter((s) => s.attention === 'needs_input').length);

	const enrollCmd = $derived(
		`cctui-daemon enroll --server-url ${apiOrigin()} --token <user-token> --name "$(hostname)"`
	);

	async function copyEnroll() {
		try {
			await navigator.clipboard.writeText(enrollCmd);
			toasts.ok('Copied');
		} catch {
			toasts.err('Clipboard unavailable');
		}
	}
</script>

<h1 class="page-title">Overview</h1>

<div class="grid">
	<div class="card stat">
		<span class="num">{live}</span><span class="lbl">Live sessions</span>
	</div>
	<div class="card stat">
		<span class="num" class:warn={needs > 0}>{needs}</span><span class="lbl">Need input</span>
	</div>
	<div class="card stat">
		<span class="num">{archived}</span><span class="lbl">Archived</span>
	</div>
	<div class="card stat">
		<span class="num">{activeUsers}</span><span class="lbl">Active users</span>
	</div>
	<div class="card stat">
		<span class="num">{revokedUsers}</span><span class="lbl">Revoked users</span>
	</div>
	<div class="card stat">
		<span class="num">{all.length}</span><span class="lbl">Total sessions</span>
	</div>
</div>

<div class="card install stack">
	<strong>Enroll a machine</strong>
	<p class="muted">
		Install <code class="mono">cctui-daemon</code> on the target host (from GitHub Releases), then
		enroll it with a user token (create one on the Users page):
	</p>
	<div class="row">
		<code class="cmd mono truncate">{enrollCmd}</code>
		<button class="btn btn-sm" onclick={copyEnroll}>Copy</button>
	</div>
	<p class="muted">
		Then run it as a service: <code class="mono">cctui-daemon service install</code>
	</p>
</div>

<div class="logout">
	<button class="btn btn-block" onclick={() => auth.clear()}>⏻ Log out</button>
</div>

<style>
	.logout {
		margin-top: var(--sp-8);
	}
	.page-title {
		font-size: var(--fs-2xl);
		margin-bottom: var(--sp-4);
	}
	.grid {
		display: grid;
		grid-template-columns: repeat(2, 1fr);
		gap: var(--sp-3);
		margin-bottom: var(--sp-4);
	}
	@media (min-width: 640px) {
		.grid {
			grid-template-columns: repeat(3, 1fr);
		}
	}
	.stat {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		align-items: flex-start;
	}
	.num {
		font-size: var(--fs-2xl);
		font-weight: var(--fw-bold);
		line-height: 1;
	}
	.num.warn {
		color: var(--warn);
	}
	.lbl {
		font-size: var(--fs-sm);
		color: var(--text-muted);
	}
	.install .cmd {
		flex: 1;
		padding: var(--sp-2) var(--sp-3);
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		font-size: var(--fs-xs);
	}
</style>
