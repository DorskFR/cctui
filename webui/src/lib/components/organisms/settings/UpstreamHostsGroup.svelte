<script lang="ts">
	import { Badge, Button, Input, Text } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import { endpoints } from '$lib/queries';
	import type { UpstreamHostsInfo } from '@bindings/UpstreamHostsInfo';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';
	import { sourceLabel } from './serverSettings.logic';

	let info = $state<UpstreamHostsInfo | null>(null);
	let draft = $state('');
	let saving = $state(false);

	$effect(() => {
		endpoints
			.upstreamHosts()
			.then((next) => (info = next))
			.catch(() => {});
	});

	async function save(hosts: string[] | null) {
		saving = true;
		try {
			info = await endpoints.setUpstreamHosts(hosts);
			draft = '';
			toasts.ok(m.settings_upstreams_saved());
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		} finally {
			saving = false;
		}
	}

	function add() {
		const entry = draft.trim();
		if (!info || !entry) return;
		save([...info.hosts, entry]);
	}
</script>

<div id="upstreams">
	<SettingGroup title={m.settings_upstreams_label()}>
		<SettingRow label={m.settings_upstreams_label()} help={m.settings_upstreams_help()} server admin wide selfLabelled>
			<div class="hosts">
				{#if info}
					{#if info.hosts.length === 0 && info.env.length === 0 && info.managed.length === 0}
						<Text size="sm" tone="faint">{m.settings_upstreams_empty()}</Text>
					{/if}
					<ul>
						{#each info.hosts as host (host)}
							<li>
								<Text size="sm" variant="code">{host}</Text>
								<Badge size="sm">{sourceLabel('settings')}</Badge>
								<Button
									size="sm"
									variant="ghost"
									disabled={saving}
									aria-label={m.settings_upstreams_remove({ host })}
									onclick={() => info && save(info.hosts.filter((h) => h !== host))}
								>
									×
								</Button>
							</li>
						{/each}
						{#each info.env.filter((h) => !info?.hosts.includes(h)) as host (host)}
							<li>
								<Text size="sm" variant="code">{host}</Text>
								<Badge size="sm">{sourceLabel('env')}</Badge>
							</li>
						{/each}
						{#each info.managed as host (host)}
							<li>
								<Text size="sm" variant="code">{host}</Text>
								<Badge size="sm">{m.settings_upstreams_managed()}</Badge>
							</li>
						{/each}
					</ul>
				{/if}
				<div class="add">
					<Input
						bind:value={draft}
						grow
						placeholder={m.settings_upstreams_add_placeholder()}
						aria-label={m.settings_upstreams_add_placeholder()}
						onkeydown={(e: KeyboardEvent) => {
							if (e.key === 'Enter' && !saving) add();
						}}
					/>
					<Button disabled={!info || !draft.trim() || saving} onclick={add}>
						{m.settings_upstreams_add()}
					</Button>
					{#if info?.source === 'settings'}
						<Button variant="ghost" disabled={saving} title={m.settings_upstreams_reset_help()} onclick={() => save(null)}>
							{m.settings_reset()}
						</Button>
					{/if}
				</div>
			</div>
		</SettingRow>
	</SettingGroup>
</div>

<style>
	.hosts {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		min-width: 0;
	}
	ul {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		margin: 0;
		padding: 0;
		list-style: none;
	}
	li,
	.add {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		flex-wrap: wrap;
	}
</style>
