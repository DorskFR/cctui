<script lang="ts">
	import { goto } from '$app/navigation';
	import { page as appPage } from '$app/state';
	import { Button, EmptyState } from '@dorsk/tsumikit';
	import PluginPageHost from '$lib/components/organisms/PluginPageHost.svelte';
	import { hostHref, pageBasePath, pluginPath, resolvePageState } from '$lib/plugins/pageRoute';
	import { pluginLoader } from '$lib/plugins/loader.svelte';
	import { usePlugins } from '$lib/queries';
	import { settings } from '$lib/settings.svelte';
	import { m } from '$lib/paraglide/messages';

	const plugins = usePlugins();
	const id = $derived(appPage.params.id ?? '');
	const basePath = $derived(pageBasePath(id));
	// SvelteKit owns the URL, so back/forward re-derive the plugin's path with no
	// popstate listener of our own.
	const path = $derived(pluginPath(basePath, appPage.url.pathname));

	const state = $derived(
		resolvePageState({
			id,
			list: plugins.data ?? [],
			enabled: settings.pluginsEnabled,
			moduleState: (info) => (info.web ? (pluginLoader.entries[info.web] ?? null) : null)
		})
	);
	const ready = $derived(state.status === 'ready' && state.info.web ? pluginLoader.entries[state.info.web] : null);

	$effect(() => {
		const info = plugins.data?.find((p) => p.id === id);
		if (info?.page && settings.pluginsEnabled[info.id] === true) pluginLoader.ensure(info);
	});

	function navigate(to: string) {
		void goto(hostHref(basePath, to), { noScroll: true, keepFocus: true });
	}
</script>

<div class="wrap" data-journey="plugin-page-route" data-plugin={id}>
	{#if plugins.isError}
		<EmptyState tone="danger" icon="warning" title={m.settings_plugins_load_failed()} />
	{:else if plugins.isPending || state.status === 'loading'}
		<EmptyState loading title={m.plugins_page_loading()} data-journey="plugin-page-loading" />
	{:else if state.status === 'ready' && ready?.status === 'ready'}
		{#key state.info.id}
			<PluginPageHost plugin={state.info} module={ready.module} {basePath} {path} {navigate} />
		{/key}
	{:else if state.status === 'failed'}
		<EmptyState
			tone="danger"
			icon="warning"
			title={m.plugins_page_failed({ name: state.info.name })}
			description={state.error}
			data-journey="plugin-page-failed"
		/>
	{:else if state.status === 'not-enabled'}
		<div class="gate" data-journey="plugin-page-not-enabled">
			<EmptyState
				icon="grid"
				title={m.plugins_page_not_enabled({ name: state.info.name })}
				description={m.plugins_page_not_enabled_help()}
			/>
			<Button size="sm" onclick={() => void goto('/settings/plugins')}>{m.plugins_page_open_settings()}</Button>
		</div>
	{:else if state.status === 'no-page'}
		<EmptyState icon="warning" title={m.plugins_page_no_page()} data-journey="plugin-page-no-page" />
	{:else}
		<EmptyState
			icon="warning"
			title={m.plugins_page_unknown({ id })}
			description={m.plugins_page_unknown_help()}
			data-journey="plugin-page-unknown"
		/>
	{/if}
</div>

<style>
	.wrap {
		display: flex;
		flex-direction: column;
		flex: 1;
		min-height: 0;
		min-width: 0;
	}
	.gate {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: var(--sp-3);
	}
</style>
