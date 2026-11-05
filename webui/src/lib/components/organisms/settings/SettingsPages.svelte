<script lang="ts">
	// Every settings area, mounted at once, one shown. Keeping them all in the
	// DOM is what lets the search box filter across areas and jump to whichever
	// one still has a match — the rows carry their own localized copy, so there
	// is no second catalogue to search. Screens embedded in an area (provider
	// accounts, users, dispatchers, the profile editor) mount only while their
	// area is shown.
	import AccountsArea from './AccountsArea.svelte';
	import InstanceArea from './InstanceArea.svelte';
	import InterfaceArea from './InterfaceArea.svelte';
	import NotificationsSection from './NotificationsSection.svelte';
	import PluginsSection from './PluginsSection.svelte';
	import SessionsArea from './SessionsArea.svelte';
	import VoiceSection from './VoiceSection.svelte';
	import type { SettingsPage } from './settings.logic';

	let { current, isAdmin }: { current: SettingsPage; isAdmin: boolean } = $props();
</script>

<div class="pg" class:on={current === 'interface'} data-settings-page="interface" data-journey="page" data-journey-key="interface">
	<InterfaceArea />
</div>
<div class="pg" class:on={current === 'accounts'} data-settings-page="accounts" data-journey="page" data-journey-key="accounts">
	<AccountsArea active={current === 'accounts'} />
</div>
<div class="pg" class:on={current === 'sessions'} data-settings-page="sessions" data-journey="page" data-journey-key="sessions">
	<SessionsArea active={current === 'sessions'} />
</div>
<div class="pg" class:on={current === 'notifications'} data-settings-page="notifications" data-journey="page" data-journey-key="notifications">
	<NotificationsSection />
	<VoiceSection />
</div>
<div class="pg" class:on={current === 'plugins'} data-settings-page="plugins" data-journey="page" data-journey-key="plugins">
	<PluginsSection />
</div>
{#if isAdmin}
	<div class="pg" class:on={current === 'instance'} data-settings-page="instance" data-journey="page" data-journey-key="instance">
		<InstanceArea active={current === 'instance'} />
	</div>
{/if}

<style>
	.pg {
		display: none;
	}
	.pg.on {
		display: flex;
		flex-direction: column;
		gap: var(--sp-8);
	}
</style>
