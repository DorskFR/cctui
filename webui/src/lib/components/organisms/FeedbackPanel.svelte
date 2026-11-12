<script lang="ts">
	import { feedbackConfig } from '$lib/config';
	import { feedbackContext, feedbackLabels } from '$lib/feedback';

	let {
		serverVersion,
		route
	}: { serverVersion: () => string | undefined; route: () => string | null } = $props();

	const config = feedbackConfig();
	const hmm = config
		? import('@dorsk/hmm').catch((error) => {
				console.warn('feedback panel disabled:', error);
				return null;
			})
		: null;
	const labels = feedbackLabels();
</script>

{#if config && hmm}
	{#await hmm then mod}
		{#if mod}
			<mod.Hmm
				endpoint={config.endpoint}
				projectKey={config.projectKey}
				position="bottom-left"
				{labels}
				context={() => feedbackContext(serverVersion(), route())}
			/>
		{/if}
	{/await}
{/if}
