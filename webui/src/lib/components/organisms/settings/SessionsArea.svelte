<script lang="ts">
	// Settings › Sessions: how sessions launch, behave and are kept private.
	import ContextSection from './ContextSection.svelte';
	import ExecutionSection from './ExecutionSection.svelte';
	import LaunchSection from './LaunchSection.svelte';
	import MacrosSection from './MacrosSection.svelte';
	import PrivacySection from './PrivacySection.svelte';
	import SessionsSection from './SessionsSection.svelte';

	let { active = false }: { active?: boolean } = $props();
</script>

<LaunchSection {active} />
<MacrosSection />
<SessionsSection />
<ContextSection />
<PrivacySection />
<ExecutionSection />
