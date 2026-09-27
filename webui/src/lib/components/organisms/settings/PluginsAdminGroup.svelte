<script lang="ts">
	import { Badge, Button, EmptyState, FileButton, Input, Switch, Text } from '@dorsk/tsumikit';
	import { useQueryClient } from '@tanstack/svelte-query';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import { endpoints, qk, useAdminPlugins } from '$lib/queries';
	import type { AdminPluginInfo } from '@bindings/AdminPluginInfo';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';

	const plugins = useAdminPlugins(() => true);
	const qc = useQueryClient();
	const list = $derived(plugins.data ?? []);

	let url = $state('');
	let busy = $state(false);

	async function run(work: () => Promise<unknown>, done: string) {
		busy = true;
		try {
			await work();
			await qc.invalidateQueries({ queryKey: qk.adminPlugins });
			await qc.invalidateQueries({ queryKey: qk.plugins });
			toasts.ok(done);
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		} finally {
			busy = false;
		}
	}

	function installFromUrl() {
		const target = url.trim();
		if (!target) return;
		void run(async () => {
			await endpoints.installPluginFromUrl(target);
			url = '';
		}, m.settings_plugins_admin_installed());
	}

	function upload(files: File[]) {
		const file = files[0];
		if (!file) return;
		void run(() => endpoints.installPluginUpload(file), m.settings_plugins_admin_installed());
	}

	function toggle(plugin: AdminPluginInfo, enabled: boolean) {
		void run(
			() => endpoints.setPluginInstanceEnabled(plugin.id, enabled),
			enabled ? m.settings_plugins_admin_enabled({ name: plugin.name }) : m.settings_plugins_admin_disabled({ name: plugin.name })
		);
	}

	function uninstall(plugin: AdminPluginInfo) {
		if (!confirm(m.settings_plugins_admin_uninstall_confirm({ name: plugin.name }))) return;
		void run(() => endpoints.uninstallPlugin(plugin.id), m.settings_plugins_admin_uninstalled({ name: plugin.name }));
	}
</script>

<div id="instance-plugins" data-journey="plugins-admin">
	<SettingGroup title={m.settings_nav_plugins()}>
		<SettingRow label={m.settings_plugins_admin_label()} help={m.settings_plugins_admin_help()} server admin wide selfLabelled>
			<div class="plugins">
				{#if plugins.isError}
					<EmptyState size="compact" tone="danger" icon="warning" title={m.settings_plugins_load_failed()} />
				{:else if plugins.isPending}
					<EmptyState size="compact" loading title={m.common_loading()} />
				{:else if list.length === 0}
					<Text size="sm" tone="faint" data-journey="plugins-admin-empty">{m.settings_plugins_admin_empty()}</Text>
				{:else}
					<ul>
						{#each list as plugin (plugin.id)}
							<li data-journey="plugin-admin-row" data-plugin={plugin.id}>
								<div class="who">
									<Text size="sm" weight="medium">{plugin.name}</Text>
									<Text size="xs" tone="faint" variant="code">{plugin.id} · {plugin.version}</Text>
								</div>
								<Badge size="sm" tone={plugin.source === 'installed' ? 'info' : 'neutral'}>
									{plugin.source === 'installed' ? m.settings_plugins_admin_source_installed() : m.settings_plugins_admin_source_directory()}
								</Badge>
								{#if plugin.source === 'installed'}
									<Switch
										bind:checked={() => plugin.enabled, (v) => toggle(plugin, v)}
										disabled={busy}
										label={m.settings_plugins_admin_enable({ name: plugin.name })}
										data-journey="plugin-admin-switch"
										data-plugin={plugin.id}
									/>
									<Button
										size="sm"
										variant="ghost"
										disabled={busy}
										aria-label={m.settings_plugins_admin_uninstall({ name: plugin.name })}
										data-journey="plugin-admin-uninstall"
										data-plugin={plugin.id}
										onclick={() => uninstall(plugin)}
									>
										×
									</Button>
								{:else}
									<Text size="xs" tone="faint">{m.settings_plugins_admin_always_on()}</Text>
								{/if}
							</li>
						{/each}
					</ul>
				{/if}
				<div class="add">
					<Input
						bind:value={url}
						grow
						placeholder={m.settings_plugins_admin_url_placeholder()}
						aria-label={m.settings_plugins_admin_url_placeholder()}
						data-journey="plugin-admin-url"
						onkeydown={(e: KeyboardEvent) => {
							if (e.key === 'Enter' && !busy) installFromUrl();
						}}
					/>
					<Button disabled={!url.trim() || busy} data-journey="plugin-admin-install" onclick={installFromUrl}>
						{m.settings_plugins_admin_install()}
					</Button>
					<FileButton label={m.settings_plugins_admin_upload()} icon="upload" accept=".tgz,.tar.gz,application/gzip" onfiles={upload} />
				</div>
			</div>
		</SettingRow>
	</SettingGroup>
</div>

<style>
	.plugins {
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
	.who {
		display: flex;
		flex-direction: column;
		min-width: 0;
		flex: 1;
	}
</style>
