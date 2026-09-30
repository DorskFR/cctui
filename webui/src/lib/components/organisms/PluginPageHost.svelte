<script lang="ts">
	import { setContext } from 'svelte';
	import { EmptyState } from '@dorsk/tsumikit';
	import { hostContext, hostUser } from '$lib/plugins/hostContext';
	import { useMe } from '$lib/queries';
	import { HOST_CONTEXT_KEY, type CctuiPluginModule, type HostContext, type PluginInfo } from '$lib/plugins/types';
	import { m } from '$lib/paraglide/messages';

	let {
		plugin,
		module,
		path,
		basePath,
		navigate
	}: {
		plugin: PluginInfo;
		module: CctuiPluginModule;
		path: string;
		basePath: string;
		navigate: (path: string) => void;
	} = $props();

	const Page = $derived(module.page);
	const me = useMe();
	// One context object per mounted plugin: the id it is keyed to decides where
	// `pluginFetch` goes, and the user only fills in once `/me` answers.
	setContext<HostContext>(HOST_CONTEXT_KEY, {
		...hostContext({ pluginId: plugin.id }),
		get user() {
			return hostUser(me.data);
		}
	});
</script>

<section class="host" aria-label={plugin.page?.title || plugin.name} data-journey="plugin-page" data-plugin={plugin.id}>
	{#if Page}
		<Page {basePath} {path} {navigate} />
	{:else}
		<EmptyState size="compact" tone="danger" icon="warning" title={m.plugins_page_no_page()} />
	{/if}
</section>

<style>
	.host {
		display: flex;
		flex-direction: column;
		flex: 1;
		min-height: 0;
		min-width: 0;
	}
</style>
