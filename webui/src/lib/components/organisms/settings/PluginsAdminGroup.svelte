<script lang="ts">
	import { Badge, Button, EmptyState, FileButton, Input, Switch, Text } from '@dorsk/tsumikit';
	import { useQueryClient } from '@tanstack/svelte-query';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import { endpoints, qk, useAdminPluginCatalog, useAdminPlugins } from '$lib/queries';
	import type { AdminPluginInfo } from '@bindings/AdminPluginInfo';
	import type { CatalogPluginInfo } from '@bindings/CatalogPluginInfo';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';

	let { isAdmin = false }: { isAdmin?: boolean } = $props();

	const plugins = useAdminPlugins(() => isAdmin);
	const catalog = useAdminPluginCatalog(() => isAdmin);
	const qc = useQueryClient();
	const list = $derived(plugins.data ?? []);
	const available = $derived(catalog.data ?? []);

	let url = $state('');
	let busy = $state(false);

	async function run(work: () => Promise<unknown>, done: string) {
		busy = true;
		try {
			await work();
			await refresh();
			toasts.ok(done);
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		} finally {
			busy = false;
		}
	}

	async function refresh() {
		await qc.invalidateQueries({ queryKey: qk.adminPlugins });
		await qc.invalidateQueries({ queryKey: qk.adminPluginCatalog });
		await qc.invalidateQueries({ queryKey: qk.plugins });
	}

	function installFromCatalog(entry: CatalogPluginInfo) {
		void run(
			() => endpoints.installPluginFromCatalog(entry.id),
			m.settings_plugins_admin_catalog_done({ name: entry.name, version: entry.version })
		);
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
	<SettingGroup title={m.settings_plugins_group_manage()}>
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
			</div>
		</SettingRow>
		<SettingRow
			label={m.settings_plugins_admin_catalog_label()}
			help={m.settings_plugins_admin_catalog_help()}
			server
			admin
			wide
			selfLabelled
		>
			<div class="plugins" data-journey="plugins-catalog">
				{#if catalog.isError}
					<EmptyState size="compact" tone="danger" icon="warning" title={m.settings_plugins_admin_catalog_failed()} />
				{:else if catalog.isPending}
					<EmptyState size="compact" loading title={m.common_loading()} />
				{:else if available.length === 0}
					<Text size="sm" tone="faint" data-journey="plugins-catalog-empty">{m.settings_plugins_admin_catalog_empty()}</Text>
				{:else}
					<ul>
						{#each available as entry (entry.id)}
							<li data-journey="plugin-catalog-row" data-plugin={entry.id}>
								<div class="who">
									<Text size="sm" weight="medium">{entry.name}</Text>
									<Text size="xs" tone="faint">{entry.description}</Text>
									<Text size="xs" tone="faint" variant="code" data-journey="plugin-catalog-version">{entry.id} · {entry.version}</Text>
								</div>
								{#if entry.homepage}
									<a href={entry.homepage} target="_blank" rel="noreferrer noopener" data-journey="plugin-catalog-homepage">
										<Text size="xs" tone="accent">{m.settings_plugins_admin_catalog_homepage()}</Text>
									</a>
								{/if}
								<Button
									size="sm"
									variant={entry.update_available ? 'primary' : 'ghost'}
									disabled={busy || (entry.installed_version !== null && !entry.update_available)}
									data-journey="plugin-catalog-install"
									data-plugin={entry.id}
									onclick={() => installFromCatalog(entry)}
								>
									{#if entry.update_available}
										{m.settings_plugins_admin_catalog_update({ version: entry.version })}
									{:else if entry.installed_version !== null}
										{m.settings_plugins_admin_catalog_up_to_date()}
									{:else}
										{m.settings_plugins_admin_catalog_install()}
									{/if}
								</Button>
							</li>
						{/each}
					</ul>
				{/if}
			</div>
		</SettingRow>
		<SettingRow
			label={m.settings_plugins_admin_manual_label()}
			help={m.settings_plugins_admin_manual_help()}
			server
			admin
			wide
			selfLabelled
		>
			<div class="plugins">
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
