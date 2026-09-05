<script lang="ts">
	import { untrack } from 'svelte';
	import { errMessage } from '$lib/api';
	import {
		useAccountActions,
		useAccountUsage,
		useSettingsCatalog,
		type AccountProvider,
		type OAuthAccount
	} from '$lib/queries';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { toasts } from '$lib/toast.svelte';
	import { providerLabel } from '$lib/providers';
	import { m } from '$lib/paraglide/messages';
	import { Button, Drawer, IconButton, NavItem, Text } from '@dorsk/tsumikit';
	import AdapterIcon from '$lib/components/atoms/AdapterIcon.svelte';
	import UsageNoticesEditor from '$lib/components/molecules/UsageNoticesEditor.svelte';
	import AnthropicProviderEditor from '$lib/components/organisms/AnthropicProviderEditor.svelte';
	import FireworksProviderEditor from '$lib/components/organisms/FireworksProviderEditor.svelte';
	import { editorWindowKeys } from '$lib/components/molecules/usage-windows';
	import { pagesFor, type PageId } from './pages.logic';
	import { knobGroups, knobKeyNames } from './knobs.logic';
	import { ProviderEdit } from './editor.svelte';
	import AliasesPage from './AliasesPage.svelte';
	import LimitsPage from './LimitsPage.svelte';
	import ModelsPage from './ModelsPage.svelte';
	import SettingsPage from './SettingsPage.svelte';
	import AdvancedPage from './AdvancedPage.svelte';

	let {
		account,
		provider,
		accounts = [],
		onclose
	}: {
		account: OAuthAccount;
		provider: AccountProvider;
		/** Same-owner move targets are picked from here. */
		accounts?: OAuthAccount[];
		onclose: () => void;
	} = $props();

	const p = untrack(() => provider);
	const kind = p.provider;
	const edit = new ProviderEdit(p);
	const pages = pagesFor(kind);

	const actions = useAccountActions();
	const qc = useQueryClient();
	const catalog = useSettingsCatalog();
	const usage = useAccountUsage(
		() => p.id,
		() => kind === 'anthropic' || kind === 'openai' || kind === 'fireworks'
	);
	const windows = $derived(usage.data?.windows ?? []);
	const softRows = $derived(
		editorWindowKeys(
			windows,
			p.soft_limits ?? null,
			edit.isFireworks ? 'fireworks' : (p.family ?? null)
		)
	);
	$effect(() => edit.seedWindows(softRows.map((r) => r.key)));

	const groups = $derived(knobGroups(catalog.data));
	const groupsOn = (id: PageId) => (edit.isAnthropic ? groups.filter((g) => g.page === id) : []);
	const claimed = $derived(new Set(groups.flatMap((g) => knobKeyNames(g.knobs)).concat(['env'])));
	const settableKeys = $derived(new Set((catalog.data?.keys ?? []).map((k) => k.name)));
	const catalogLoading = $derived(edit.isAnthropic && !catalog.data && !catalog.error);
	const catalogFailed = $derived(edit.isAnthropic && !!catalog.error);
	const changes = $derived(edit.changes(groupsOn(edit.page)));

	const LABELS: Record<PageId, () => string> = {
		aliases: m.provider_page_aliases,
		limits: m.provider_page_limits,
		ui: m.provider_page_ui,
		privacy: m.provider_page_privacy,
		tools: m.provider_page_tools,
		gateway: m.provider_page_gateway,
		models: m.provider_page_models,
		advanced: m.provider_page_advanced
	};
	const title = $derived(
		m.provider_drawer_title({ provider: providerLabel(kind), account: account.name })
	);

	const moveTargets = $derived(
		accounts
			.filter(
				(a) =>
					a.id !== account.id &&
					a.user_id === account.user_id &&
					!a.providers.some((x) => x.family === p.family)
			)
			.map((a) => ({ id: a.id, name: a.name }))
	);

	async function move(targetId: string) {
		try {
			await actions.moveProvider(account.id, p.id, targetId);
			toasts.ok(m.accounts_provider_moved());
			onclose();
		} catch (e) {
			toasts.error(errMessage(e));
		}
	}

	async function save() {
		try {
			await actions.updateProvider(account.id, p.id, edit.body());
			toasts.ok(m.accounts_provider_updated());
			onclose();
		} catch (e) {
			toasts.error(errMessage(e));
		}
	}
</script>

<Drawer
	side="right"
	width="620px"
	navWidth="150px"
	{title}
	page={LABELS[edit.page]()}
	{onclose}
	closeLabel={m.common_close()}
>
	{#snippet header()}
		<div class="head">
			<span class="mark"><AdapterIcon provider={kind} size={20} /></span>
			<div class="titles">
				<Text as="div" size="md" weight="semibold">{title}</Text>
				<Text as="div" size="xs" tone="faint">{LABELS[edit.page]()}</Text>
			</div>
			<span class="spacer"></span>
			<IconButton icon="x" label={m.common_close()} variant="ghost" onclick={onclose} />
		</div>
	{/snippet}

	{#snippet nav()}
		{#each pages as id (id)}
			<NavItem label={LABELS[id]()} active={edit.page === id} activeStyle="bar" onclick={() => (edit.page = id)} />
		{/each}
	{/snippet}

	<div class="pane">
		{#if edit.page === 'aliases'}
			<AliasesPage bind:rows={edit.aliasRows} models={edit.models} />
		{:else if edit.page === 'limits'}
			<LimitsPage
				rows={softRows}
				{windows}
				bind:edits={edit.soft}
				bind:rate={edit.rate}
			/>
		{:else if edit.page === 'models'}
			<ModelsPage
				fireworks={edit.isFireworks}
				bind:models={edit.models}
				bind:settings={edit.providerSettings}
			/>
		{:else}
			<SettingsPage
				groups={groupsOn(edit.page)}
				bind:settings={edit.settings}
				preset={catalog.data?.preset}
				loading={catalogLoading}
				failed={catalogFailed}
			>
				{#if edit.page === 'ui' && edit.isAnthropic}
					<AnthropicProviderEditor bind:settings={edit.providerSettings} />
				{:else if edit.page === 'gateway'}
					{#if kind === 'anthropic' || kind === 'openai'}
						<UsageNoticesEditor bind:value={edit.notices} />
					{/if}
					{#if edit.isFireworks}
						<FireworksProviderEditor
							section="gateway"
							bind:settings={edit.providerSettings}
							bind:models={edit.models}
						/>
					{/if}
				{:else if edit.page === 'advanced'}
					<AdvancedPage
						endpoint={edit.isFireworks || edit.isCompatible}
						rawJson={edit.isAnthropic}
						{settableKeys}
						catalogReady={!!catalog.data}
						{claimed}
						bind:baseUrl={edit.baseUrl}
						bind:credential={edit.credential}
						bind:authScheme={edit.authScheme}
						bind:settings={edit.settings}
						{moveTargets}
						moveFamily={p.family}
						onmove={move}
					/>
				{/if}
			</SettingsPage>
		{/if}
	</div>

	{#snippet footer()}
		<Text as="span" size="xs" tone="faint">
			{changes === 0 ? m.provider_drawer_no_changes() : m.provider_drawer_changes({ n: changes })}
		</Text>
		<span class="spacer"></span>
		<Button size="sm" onclick={onclose}>{m.common_cancel()}</Button>
		<Button size="sm" variant="primary" onclick={save}>{m.common_save()}</Button>
	{/snippet}
</Drawer>

<style>
	.head {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		padding: var(--sp-3) var(--sp-4);
		border-bottom: 1px solid var(--border);
	}
	.mark {
		display: inline-flex;
		flex: none;
	}
	.titles {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}
	.spacer {
		flex: 1;
	}
	.pane {
		container-type: inline-size;
	}
</style>
