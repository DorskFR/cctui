<script lang="ts">
	import { updated } from '$app/state';
	import { ws } from '#lib/ws.svelte.ts';
	import { Text } from '@dorsk/tsumikit';
	import NavLink from '#lib/components/atoms/NavLink.svelte';
	import ProviderStatusAlert from '#lib/components/molecules/ProviderStatusAlert.svelte';
	import { m } from '#lib/paraglide/messages.js';
</script>

<div class="lead">
	<NavLink href="/sessions" title={m.nav_sessions()}>
		<div class="brand">
			<Text variant="code" tone="accent" size="lg" weight="bold">»_</Text>
			<Text size="lg" weight="bold">cctui</Text>
		</div>
	</NavLink>
	<span
		class="conn"
		class:on={ws.status === 'open'}
		class:mid={ws.status === 'connecting'}
		title={m.nav_ws_status({ status: ws.status })}
	></span>
	<ProviderStatusAlert />
	{#if updated.current}
		<button
			type="button"
			class="reload"
			title={m.reload_new_ui_title()}
			onclick={() => location.reload()}
		>
			<Text size="xs" tone="accent" weight="semibold" nowrap>{m.version_chip_reload()}</Text>
		</button>
	{/if}
</div>

<style>
	.lead {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.brand {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.conn {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--dot-dead);
		flex: none;
	}
	.conn.on {
		background: var(--ok);
		box-shadow: 0 0 6px var(--ok);
	}
	.conn.mid {
		background: var(--warn);
	}
	.reload {
		border: 0;
		flex: none;
		height: 24px;
		padding: 0 var(--sp-2);
		border-radius: var(--r-pill);
		background: var(--bg-elevated-2);
		cursor: pointer;
		font: inherit;
	}
	@media (pointer: coarse) {
		.reload {
			min-height: 44px;
		}
	}
</style>
