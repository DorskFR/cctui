<script lang="ts">
	import { ws } from '$lib/ws.svelte';
	import { useVersion, useSessions } from '$lib/queries';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { theme } from '$lib/theme.svelte';
	import { notify } from '$lib/notify.svelte';
	import { toasts } from '$lib/toast.svelte';

	const version = useVersion();

	// Global "needs input" watcher (CCT-170). Header is always mounted inside
	// the query provider, so it's the natural home for the cross-route watcher.
	const qc = useQueryClient();
	const sessions = useSessions(() => false);

	// Live ws changes → refetch the list even on routes other than /sessions.
	$effect(() => {
		void ws.changeTick;
		qc.invalidateQueries({ queryKey: ['sessions'] });
	});

	// Drive notifications + title badge off the list's attention flags.
	$effect(() => {
		const items = $sessions.data?.sessions ?? [];
		notify.reconcile(items.filter((s) => s.attention === 'needs_input'));
	});

	async function toggleNotify() {
		if (notify.enabled) {
			notify.disable();
			toasts.push('Notifications off', 'info');
			return;
		}
		if (!notify.supported) {
			toasts.err('Notifications not supported in this browser');
			return;
		}
		const ok = await notify.enable();
		if (ok) toasts.ok('Notifications on');
		else toasts.err('Notifications blocked by browser');
	}
</script>

<header class="hd">
	<div class="hd-inner container">
		<div class="brand">
			<span class="logo">»_</span>
			<span class="name">cctui</span>
		</div>
		<span
			class="conn"
			class:on={ws.status === 'open'}
			class:mid={ws.status === 'connecting'}
			title={`websocket: ${ws.status}`}
		></span>
		<div class="spacer"></div>
		{#if $version.data}
			<a class="ver mono" href={$version.data.commit_url} target="_blank" rel="noopener">
				srv v{$version.data.version} · ui v{__CLIENT_VERSION__}
			</a>
		{/if}
		<button
			class="btn btn-ghost btn-icon"
			class:notify-on={notify.enabled}
			title={notify.enabled ? 'Notifications on — click to mute' : 'Notify me when a session needs input'}
			onclick={toggleNotify}
			oncontextmenu={(e) => {
				e.preventDefault();
				notify.setSound(!notify.sound);
				toasts.push(notify.sound ? 'Sound on' : 'Sound off', 'info');
			}}
		>
			{notify.enabled ? '🔔' : '🔕'}
		</button>
		<button class="btn btn-ghost btn-icon" title="Toggle theme ({theme.current})" onclick={() => theme.toggle()}>
			{theme.icon}
		</button>
	</div>
</header>

<style>
	.hd {
		position: fixed;
		top: 0;
		left: 0;
		right: 0;
		z-index: var(--z-header);
		background: color-mix(in srgb, var(--bg-elevated) 92%, transparent);
		backdrop-filter: blur(8px);
		border-bottom: 1px solid var(--border);
		padding-top: var(--safe-top);
	}
	.hd-inner {
		height: var(--header-h);
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.brand {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		font-weight: var(--fw-bold);
		font-size: var(--fs-lg);
	}
	.logo {
		font-family: var(--font-mono);
		color: var(--accent);
	}
	.conn {
		width: 0.5rem;
		height: 0.5rem;
		border-radius: 50%;
		background: var(--dot-dead);
	}
	.conn.on {
		background: var(--ok);
		box-shadow: 0 0 6px var(--ok);
	}
	.conn.mid {
		background: var(--warn);
	}
	.ver {
		font-size: var(--fs-xs);
		color: var(--text-faint);
	}
	.notify-on {
		color: var(--accent);
	}
</style>
