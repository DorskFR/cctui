<script lang="ts">
	// Every settings page, mounted at once, one shown. Keeping them all in the
	// DOM is what lets the search box filter across pages and jump to whichever
	// one still has a match — the rows carry their own localized copy, so there
	// is no second catalogue to search.
	import AppearanceSection from './AppearanceSection.svelte';
	import SessionsSection from './SessionsSection.svelte';
	import ExecutionSection from './ExecutionSection.svelte';
	import PrivacySection from './PrivacySection.svelte';
	import NotificationsSection from './NotificationsSection.svelte';
	import MonitoringSection from './MonitoringSection.svelte';
import SecuritySection from './SecuritySection.svelte';
	import InstanceSection from './InstanceSection.svelte';
	import type { SettingsPage } from './settings.logic';

	let { current, isAdmin }: { current: SettingsPage; isAdmin: boolean } = $props();
</script>

<div class="pg" class:on={current === 'appearance'} data-settings-page="appearance">
	<AppearanceSection />
</div>
<div class="pg" class:on={current === 'sessions'} data-settings-page="sessions">
	<SessionsSection />
</div>
<div class="pg" class:on={current === 'execution'} data-settings-page="execution">
	<ExecutionSection />
</div>
<div class="pg" class:on={current === 'privacy'} data-settings-page="privacy">
	<PrivacySection />
</div>
<div class="pg" class:on={current === 'notifications'} data-settings-page="notifications">
	<NotificationsSection />
</div>
<div class="pg" class:on={current === 'monitoring'} data-settings-page="monitoring">
	<MonitoringSection />
</div>
<div class="pg" class:on={current === 'security'} data-settings-page="security">
	<SecuritySection {isAdmin} />
</div>
<div class="pg" class:on={current === 'instance'} data-settings-page="instance">
	<InstanceSection {isAdmin} />
</div>

<style>
	.pg {
		display: none;
	}
	.pg.on {
		display: block;
	}
</style>
