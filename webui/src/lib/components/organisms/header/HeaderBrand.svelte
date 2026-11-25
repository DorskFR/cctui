<script lang="ts">
	import { updated } from '$app/state';
	import { ws } from '#lib/ws.svelte.ts';
	import { useVersion } from '#lib/queries/index.ts';
	import { releaseChannel } from '#lib/releaseChannel.ts';
	import { Text } from '@dorsk/tsumikit';
	import NavLink from '#lib/components/atoms/NavLink.svelte';
	import { m } from '#lib/paraglide/messages.js';

	const version = useVersion();
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
	<span class="vers">
		<Text size="xs" tone="faint" variant="code">ui v{__CLIENT_VERSION__}</Text>
		{#if version.data}
			<NavLink href={version.data.commit_url} target="_blank" rel="noopener">
				<Text size="xs" tone="faint" variant="code">srv v{version.data.version}</Text>
			</NavLink>
			{#if releaseChannel(version.data.version) === 'beta'}
				<span title={m.release_channel_beta_title()}>
					<Text size="xs" tone="accent" variant="code" weight="bold">{m.release_channel_beta()}</Text>
				</span>
			{/if}
		{/if}
		{#if updated.current}
			<button
				type="button"
				class="reload"
				title={m.reload_new_ui_title()}
				onclick={() => location.reload()}
			>
				<Text size="xs" tone="accent" variant="code" weight="bold">{m.reload_new_ui_hint()}</Text>
			</button>
		{/if}
	</span>
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
	/* Two stacked lines cost no more width than one; below the nav breakpoint
	   the block cannot fit, so it goes and the account menu carries it. */
	.vers {
		display: none;
		flex-direction: column;
		align-items: flex-start;
		flex: none;
		line-height: 1;
		white-space: nowrap;
	}
	@media (min-width: 48rem) {
		.vers {
			display: flex;
		}
	}
	.reload {
		border: 0;
		padding: 0;
		background: none;
		cursor: pointer;
		font: inherit;
		text-decoration: underline;
	}
</style>
