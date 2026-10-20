<script lang="ts">
	import { untrack } from 'svelte';
	import { errMessage } from '$lib/api';
	import {
		useAccountActions,
		useAccountUsage,
		useLimitReset,
		useSettingsCatalog,
		type AccountProvider,
		type LimitResetEntry,
		type OAuthAccount
	} from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';
	import { Button, MasterDetail, NavItem, Text } from '@dorsk/tsumikit';
	import UsageNoticesEditor from '$lib/components/molecules/UsageNoticesEditor.svelte';
	import AutoLimitResetEditor from '$lib/components/molecules/AutoLimitResetEditor.svelte';
	import FireworksProviderEditor from '$lib/components/organisms/FireworksProviderEditor.svelte';
	import { editorWindowKeys } from '$lib/components/molecules/usage-windows';
	import { pagesFor, type PageId } from './pages.logic';
	import { knobGroups, knobKeyNames } from './knobs.logic';
	import { ProviderEdit } from './editor.svelte';
	import AliasesPage from './AliasesPage.svelte';
	import LimitsPage from './LimitsPage.svelte';
	import LimitResetsPage from './LimitResetsPage.svelte';
	import ModelsPage from './ModelsPage.svelte';
	import SettingsPage from './SettingsPage.svelte';
	import AdvancedPage from './AdvancedPage.svelte';

	let {
		account,
		provider,
		accounts = [],
		initialPage,
		onmoved
	}: {
		account: OAuthAccount;
		provider: AccountProvider;
		/** Same-owner move targets are picked from here. */
		accounts?: OAuthAccount[];
		/** Deep-link target; ignored when this provider has no such page. */
		initialPage?: PageId;
		onmoved: () => void;
	} = $props();

	let p = $state.raw(untrack(() => provider));
	const kind = untrack(() => p.provider);
	const pages = pagesFor(kind);
	const landing = untrack(() => initialPage);

	function seed(from: AccountProvider, page: PageId | undefined): ProviderEdit {
		const e = new ProviderEdit(from);
		if (page && pages.includes(page)) e.page = page;
		return e;
	}
	let edit = $state(seed(untrack(() => provider), landing));
	let picked = $state(!!landing && pages.includes(landing));

	const actions = useAccountActions();
	const catalog = useSettingsCatalog(() => p.family ?? 'anthropic');
	const usage = useAccountUsage(
		() => p.id,
		() => kind === 'anthropic' || kind === 'openai' || kind === 'fireworks' || kind === 'google'
	);
	const windows = $derived(usage.data?.windows ?? []);
	const softRows = $derived(
		editorWindowKeys(
			windows,
			p.soft_limits ?? null,
			edit.isFireworks ? 'fireworks' : (p.family ?? null),
			!!p.usage_probe
		)
	);
	$effect(() => edit.seedWindows(softRows.map((r) => r.key)));

	const groups = $derived(knobGroups(catalog.data));
	const groupsOn = (id: PageId) => (edit.hasCatalog ? groups.filter((g) => g.page === id) : []);
	const claimed = $derived(new Set(groups.flatMap((g) => knobKeyNames(g.knobs)).concat(['env'])));
	const settableKeys = $derived(new Set((catalog.data?.keys ?? []).map((k) => k.name)));
	const blockedKeys = $derived(new Set(catalog.data?.blocked ?? []));
	const catalogLoading = $derived(edit.hasCatalog && !catalog.data && !catalog.error);
	const catalogFailed = $derived(edit.hasCatalog && !!catalog.error);
	const changes = $derived(edit.changes(groupsOn(edit.page)));

	const LABELS: Record<PageId, () => string> = {
		aliases: m.provider_page_aliases,
		limits: m.provider_page_limits,
		resets: m.provider_page_resets,
		ui: m.provider_page_ui,
		privacy: m.provider_page_privacy,
		tools: m.provider_page_tools,
		speed: m.provider_page_speed,
		reasoning: m.provider_page_reasoning,
		gateway: m.provider_page_gateway,
		models: m.provider_page_models,
		advanced: m.provider_page_advanced
	};

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
			onmoved();
		} catch (e) {
			toasts.error(errMessage(e));
		}
	}

	const claimReset = useLimitReset();
	let claiming = $state<string | null>(null);

	async function claim(entry: LimitResetEntry) {
		if (claiming) return;
		claiming = entry.id;
		try {
			const r = await claimReset(p.id, entry.id);
			const text = m.sessions_limit_reset_outcome({ outcome: r.outcome });
			if (r.outcome === 'reset') toasts.ok(text);
			else toasts.error(text);
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			claiming = null;
		}
	}

	async function save() {
		try {
			p = await actions.updateProvider(account.id, p.id, edit.body());
			edit = seed(p, edit.page);
			toasts.ok(m.accounts_provider_updated());
		} catch (e) {
			toasts.error(errMessage(e));
		}
	}

	const discard = () => (edit = seed(p, edit.page));
</script>

<div class="settings">
	<MasterDetail
		listWidth="10rem"
		gap="var(--sp-3)"
		selected={picked}
		onback={() => (picked = false)}
		backLabel={m.drawer_back()}
		listLabel={m.account_section_provider_settings()}
	>
		{#snippet list()}
			<nav class="nav">
				{#each pages as id (id)}
					<NavItem
						label={LABELS[id]()}
						active={edit.page === id}
						activeStyle="bar"
						onclick={() => {
							edit.page = id;
							picked = true;
						}}
					/>
				{/each}
			</nav>
		{/snippet}
		{#snippet detailHeader()}
			<Text as="div" size="sm" weight="semibold">{LABELS[edit.page]()}</Text>
		{/snippet}
		{#snippet detail()}
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
					{#if kind === 'anthropic' || kind === 'openai'}
						<AutoLimitResetEditor bind:value={edit.autoReset} family={kind} />
						<UsageNoticesEditor bind:value={edit.notices} />
					{/if}
				{:else if edit.page === 'resets'}
					<LimitResetsPage entries={usage.data?.limit_resets ?? []} {claiming} onclaim={claim} />
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
						{#if edit.page === 'gateway'}
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
								{blockedKeys}
								catalogReady={!!catalog.data}
								{claimed}
								bind:baseUrl={edit.baseUrl}
								bind:credential={edit.credential}
								bind:authScheme={edit.authScheme}
								bind:usageProbe={edit.usageProbe}
								bind:settings={edit.settings}
								{moveTargets}
								moveFamily={p.family}
								onmove={move}
							/>
						{/if}
					</SettingsPage>
				{/if}
			</div>
		{/snippet}
	</MasterDetail>

	<div class="foot">
		<Text as="span" size="xs" tone="faint">
			{changes === 0 ? m.provider_drawer_no_changes() : m.provider_drawer_changes({ n: changes })}
		</Text>
		<span class="spacer"></span>
		<Button size="sm" onclick={discard}>{m.common_reset()}</Button>
		<Button size="sm" variant="primary" onclick={save}>{m.common_save()}</Button>
	</div>
</div>

<style>
	.settings {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.nav {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	.pane {
		container-type: inline-size;
	}
	.foot {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: var(--sp-2);
		padding-top: var(--sp-3);
		border-top: 1px solid var(--border);
	}
	.spacer {
		flex: 1;
	}
</style>
